use crate::manifest::{CommitState, ObjectManifest};
use crate::multipart::{MultipartPart, MultipartSession, MultipartState};
use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::str::FromStr;
use std::sync::Mutex;
use thiserror::Error;
use time::OffsetDateTime;
use time::format_description::well_known::Rfc3339;
use uuid::Uuid;

const SCHEMA_VERSION: u32 = 4;

#[derive(Debug, Error)]
pub enum MetadataError {
    #[error("sqlite error: {0}")]
    Sqlite(#[from] rusqlite::Error),
    #[error("serialization error: {0}")]
    Serde(#[from] serde_json::Error),
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("unsupported schema version {0}")]
    UnsupportedSchemaVersion(u32),
    #[error("manifest not found: {0}")]
    ManifestNotFound(String),
    #[error("journal entry not found: {0}")]
    JournalNotFound(String),
    #[error("multipart session not found: {0}")]
    MultipartSessionNotFound(String),
    #[error("invalid manifest: {0}")]
    InvalidManifest(String),
    #[error("invalid operation kind: {0}")]
    InvalidOperationKind(String),
    #[error("bucket not found: {0}")]
    BucketNotFound(String),
    #[error("bucket already exists: {0}")]
    BucketAlreadyExists(String),
    #[error("bucket not empty: {0}")]
    BucketNotEmpty(String),
    #[error("metadata state is poisoned")]
    Poisoned,
    #[error("time formatting error: {0}")]
    TimeFormat(#[from] time::error::Format),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OperationKind {
    Put,
    Delete,
    Rebuild,
    Repair,
    GarbageCollect,
}

impl OperationKind {
    fn as_str(self) -> &'static str {
        match self {
            OperationKind::Put => "put",
            OperationKind::Delete => "delete",
            OperationKind::Rebuild => "rebuild",
            OperationKind::Repair => "repair",
            OperationKind::GarbageCollect => "garbage_collect",
        }
    }
}

impl FromStr for OperationKind {
    type Err = MetadataError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "put" => Ok(OperationKind::Put),
            "delete" => Ok(OperationKind::Delete),
            "rebuild" => Ok(OperationKind::Rebuild),
            "repair" => Ok(OperationKind::Repair),
            "garbage_collect" => Ok(OperationKind::GarbageCollect),
            _ => Err(MetadataError::InvalidOperationKind(value.to_string())),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MetadataStatus {
    pub path: Option<PathBuf>,
    pub schema_version: u32,
    pub buckets: u64,
    pub committed_objects: u64,
    pub active_objects: u64,
    pub staged_objects: u64,
    pub recovery_markers: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BucketRecord {
    pub name: String,
    pub created_at: OffsetDateTime,
    pub deleted_at: Option<OffsetDateTime>,
    pub versioning_enabled: bool,
    pub object_locking_enabled: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RebuildReport {
    pub committed_rows: u64,
    pub active_rows: u64,
    pub staged_rows: u64,
    pub recovery_markers: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VerifyReport {
    pub expected_active_rows: u64,
    pub actual_active_rows: u64,
    pub mismatched_rows: u64,
    pub staged_rows: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TombstonedManifestRecord {
    pub manifest: ObjectManifest,
    pub tombstoned_at: OffsetDateTime,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JournalEntry {
    pub operation_id: Uuid,
    pub object_id: Uuid,
    pub bucket: String,
    pub object_key: String,
    pub operation_kind: OperationKind,
    pub state: String,
    pub reason: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct JournalDetails {
    operation_id: String,
    object_id: String,
    bucket: String,
    object_key: String,
    operation_kind: OperationKind,
    state: String,
    reason: Option<String>,
}

pub struct MetadataStore {
    path: Option<PathBuf>,
    connection: Mutex<Connection>,
}

impl MetadataStore {
    pub fn open(path: impl AsRef<Path>) -> Result<Self, MetadataError> {
        let path = path.as_ref().to_path_buf();
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        create_private_metadata_file(&path)?;

        let connection = Connection::open(&path)?;
        connection.pragma_update(None, "foreign_keys", true)?;

        let store = Self {
            path: Some(path),
            connection: Mutex::new(connection),
        };
        store.with_connection(|connection| {
            apply_migrations(connection)?;
            rebuild_index_internal(connection)?;
            Ok(())
        })?;
        Ok(store)
    }

    pub fn open_in_memory() -> Result<Self, MetadataError> {
        let connection = Connection::open_in_memory()?;
        connection.pragma_update(None, "foreign_keys", true)?;

        let store = Self {
            path: None,
            connection: Mutex::new(connection),
        };
        store.with_connection(|connection| {
            apply_migrations(connection)?;
            rebuild_index_internal(connection)?;
            Ok(())
        })?;
        Ok(store)
    }

    pub fn path(&self) -> Option<&Path> {
        self.path.as_deref()
    }

    pub fn schema_version(&self) -> Result<u32, MetadataError> {
        self.with_connection(|connection| read_schema_version(connection))
    }

    pub fn status(&self) -> Result<MetadataStatus, MetadataError> {
        self.with_connection(|connection| {
            Ok(MetadataStatus {
                path: self.path.clone(),
                schema_version: read_schema_version(connection)?,
                buckets: count_rows(
                    connection,
                    "SELECT COUNT(*) FROM buckets WHERE deleted_at IS NULL",
                )?,
                committed_objects: count_rows(
                    connection,
                    "SELECT COUNT(*) FROM object_manifests WHERE commit_state = 'committed'",
                )?,
                active_objects: count_rows(connection, "SELECT COUNT(*) FROM active_objects")?,
                staged_objects: count_rows(
                    connection,
                    "SELECT COUNT(*) FROM object_manifests WHERE commit_state = 'staging'",
                )?,
                recovery_markers: count_rows(connection, "SELECT COUNT(*) FROM recovery_markers")?,
            })
        })
    }

    pub fn migrate(&self) -> Result<u32, MetadataError> {
        self.with_connection(|connection| {
            apply_migrations(connection)?;
            read_schema_version(connection)
        })
    }

    pub fn create_bucket(&self, bucket: BucketRecord) -> Result<BucketRecord, MetadataError> {
        if bucket.name.trim().is_empty() {
            return Err(MetadataError::InvalidManifest(
                "bucket name is required".to_string(),
            ));
        }
        let BucketRecord {
            name,
            versioning_enabled,
            object_locking_enabled,
            ..
        } = bucket;

        self.with_connection(|connection| {
            let tx = connection.transaction()?;
            let existing = tx
                .query_row(
                    r#"
                    SELECT name, created_at, deleted_at, versioning_enabled, object_locking_enabled
                    FROM buckets
                    WHERE name = ?1
                    "#,
                    params![name.clone()],
                    |row| {
                        Ok((
                            row.get::<_, String>(0)?,
                            row.get::<_, String>(1)?,
                            row.get::<_, Option<String>>(2)?,
                            row.get::<_, i64>(3)?,
                            row.get::<_, i64>(4)?,
                        ))
                    },
                )
                .optional()?;

            let now = timestamp_now()?;
            match existing {
                Some((name, _, None, _, _)) => Err(MetadataError::BucketAlreadyExists(name)),
                Some((name, _created_at, Some(_), _, _)) => {
                    tx.execute(
                        r#"
                        UPDATE buckets
                        SET created_at = ?2,
                            deleted_at = NULL,
                            versioning_enabled = ?3,
                            object_locking_enabled = ?4
                        WHERE name = ?1
                        "#,
                        params![
                            name.clone(),
                            now.clone(),
                            versioning_enabled,
                            object_locking_enabled
                        ],
                    )?;
                    tx.commit()?;
                    Ok(BucketRecord {
                        name,
                        created_at: parse_rfc3339_timestamp(&now)?,
                        deleted_at: None,
                        versioning_enabled,
                        object_locking_enabled,
                    })
                }
                None => {
                    tx.execute(
                        r#"
                        INSERT INTO buckets (
                            name,
                            created_at,
                            deleted_at,
                            versioning_enabled,
                            object_locking_enabled
                        )
                        VALUES (?1, ?2, NULL, ?3, ?4)
                        "#,
                        params![
                            name.clone(),
                            now.clone(),
                            versioning_enabled,
                            object_locking_enabled
                        ],
                    )?;
                    tx.commit()?;
                    Ok(BucketRecord {
                        name,
                        created_at: parse_rfc3339_timestamp(&now)?,
                        deleted_at: None,
                        versioning_enabled,
                        object_locking_enabled,
                    })
                }
            }
        })
    }

    pub fn get_bucket(&self, bucket: &str) -> Result<Option<BucketRecord>, MetadataError> {
        self.with_connection(|connection| load_bucket_record(connection, bucket, false))
    }

    pub fn list_buckets(&self) -> Result<Vec<BucketRecord>, MetadataError> {
        self.with_connection(|connection| {
            let mut stmt = connection.prepare(
                r#"
                SELECT name, created_at, deleted_at, versioning_enabled, object_locking_enabled
                FROM buckets
                WHERE deleted_at IS NULL
                ORDER BY name ASC
                "#,
            )?;
            let mut buckets = Vec::new();
            let mut rows = stmt.query([])?;
            while let Some(row) = rows.next()? {
                let created_at = row.get::<_, String>(1)?;
                let deleted_at = row.get::<_, Option<String>>(2)?;
                buckets.push(BucketRecord {
                    name: row.get(0)?,
                    created_at: parse_rfc3339_timestamp(&created_at)?,
                    deleted_at: match deleted_at {
                        Some(value) => Some(parse_rfc3339_timestamp(&value)?),
                        None => None,
                    },
                    versioning_enabled: row.get::<_, i64>(3)? != 0,
                    object_locking_enabled: row.get::<_, i64>(4)? != 0,
                });
            }
            Ok(buckets)
        })
    }

    pub fn delete_bucket(&self, bucket: &str) -> Result<(), MetadataError> {
        self.with_connection(|connection| {
            let tx = connection.transaction()?;
            let active_objects: u64 = tx.query_row(
                "SELECT COUNT(*) FROM active_objects WHERE bucket = ?1",
                params![bucket],
                |row| row.get::<_, u64>(0),
            )?;
            let pending_objects: u64 = tx.query_row(
                "SELECT COUNT(*) FROM object_manifests WHERE bucket = ?1 AND commit_state IN ('committed', 'staging', 'orphaned', 'recovery_required')",
                params![bucket],
                |row| row.get::<_, u64>(0),
            )?;
            if active_objects > 0 || pending_objects > 0 {
                return Err(MetadataError::BucketNotEmpty(bucket.to_string()));
            }

            let updated = tx.execute(
                r#"
                UPDATE buckets
                SET deleted_at = ?2
                WHERE name = ?1 AND deleted_at IS NULL
                "#,
                params![bucket, timestamp_now()?],
            )?;
            if updated == 0 {
                return Err(MetadataError::BucketNotFound(bucket.to_string()));
            }

            tx.commit()?;
            Ok(())
        })
    }

    pub fn bucket_exists(&self, bucket: &str) -> Result<bool, MetadataError> {
        Ok(self.get_bucket(bucket)?.is_some())
    }

    pub fn list_bucket_manifests(
        &self,
        bucket: &str,
        prefix: Option<&str>,
    ) -> Result<Vec<ObjectManifest>, MetadataError> {
        self.with_connection(|connection| {
            let mut sql = String::from(
                r#"
                SELECT m.manifest_json
                FROM active_objects a
                JOIN object_manifests m ON m.object_id = a.object_id
                WHERE a.bucket = ?1
                "#,
            );
            if prefix.is_some() {
                sql.push_str(" AND a.object_key LIKE ?2 || '%'");
            }
            sql.push_str(" ORDER BY a.object_key ASC, a.object_id ASC");

            let mut stmt = connection.prepare(&sql)?;
            let mut manifests = Vec::new();
            if let Some(prefix) = prefix {
                let mut rows = stmt.query(params![bucket, prefix])?;
                while let Some(row) = rows.next()? {
                    let json = row.get::<_, String>(0)?;
                    let manifest = serde_json::from_str::<ObjectManifest>(&json)?;
                    manifest
                        .validate()
                        .map_err(MetadataError::InvalidManifest)?;
                    manifests.push(manifest);
                }
            } else {
                let mut rows = stmt.query(params![bucket])?;
                while let Some(row) = rows.next()? {
                    let json = row.get::<_, String>(0)?;
                    let manifest = serde_json::from_str::<ObjectManifest>(&json)?;
                    manifest
                        .validate()
                        .map_err(MetadataError::InvalidManifest)?;
                    manifests.push(manifest);
                }
            }
            Ok(manifests)
        })
    }

    pub fn stage_manifest(
        &self,
        operation_kind: OperationKind,
        manifest: ObjectManifest,
    ) -> Result<Uuid, MetadataError> {
        manifest
            .validate()
            .map_err(MetadataError::InvalidManifest)?;

        let operation_id = Uuid::new_v4();
        let created_at = timestamp_now()?;
        let object_id = manifest.object_id.to_string();
        let bucket = manifest.bucket.clone();
        let object_key = manifest.key.clone();
        let version_id = manifest.version_id.clone();
        let manifest_json = manifest_json_with_state(&manifest, CommitState::Staging)?;
        let journal_details = JournalDetails {
            operation_id: operation_id.to_string(),
            object_id: object_id.clone(),
            bucket: bucket.clone(),
            object_key: object_key.clone(),
            operation_kind,
            state: "staging".to_string(),
            reason: None,
        };
        let journal_json = serde_json::to_string(&journal_details)?;

        self.with_connection(|connection| {
            let tx = connection.transaction()?;
            tx.execute(
                r#"
                INSERT INTO object_manifests (
                    object_id,
                    bucket,
                    object_key,
                    version_id,
                    commit_state,
                    manifest_json,
                    created_at,
                    committed_at,
                    tombstoned_at
                )
                VALUES (?1, ?2, ?3, ?4, 'staging', ?5, ?6, NULL, NULL)
                ON CONFLICT(object_id) DO UPDATE SET
                    bucket = excluded.bucket,
                    object_key = excluded.object_key,
                    version_id = excluded.version_id,
                    commit_state = excluded.commit_state,
                    manifest_json = excluded.manifest_json,
                    created_at = excluded.created_at,
                    committed_at = NULL,
                    tombstoned_at = NULL
                "#,
                params![
                    object_id,
                    bucket,
                    object_key,
                    version_id,
                    manifest_json,
                    created_at
                ],
            )?;
            tx.execute(
                r#"
                INSERT INTO operation_journal (
                    operation_id,
                    object_id,
                    bucket,
                    object_key,
                    operation_kind,
                    state,
                    manifest_json,
                    error,
                    created_at,
                    updated_at
                )
                VALUES (?1, ?2, ?3, ?4, ?5, 'staging', ?6, NULL, ?7, ?7)
                ON CONFLICT(operation_id) DO UPDATE SET
                    object_id = excluded.object_id,
                    bucket = excluded.bucket,
                    object_key = excluded.object_key,
                    operation_kind = excluded.operation_kind,
                    state = excluded.state,
                    manifest_json = excluded.manifest_json,
                    error = NULL,
                    updated_at = excluded.updated_at
                "#,
                params![
                    journal_details.operation_id,
                    journal_details.object_id,
                    journal_details.bucket,
                    journal_details.object_key,
                    operation_kind.as_str(),
                    journal_json,
                    created_at,
                ],
            )?;
            tx.commit()?;
            Ok(())
        })?;

        self.ensure_recovery_marker(&journal_details)?;
        Ok(operation_id)
    }

    pub fn commit_manifest(&self, operation_id: Uuid) -> Result<ObjectManifest, MetadataError> {
        let operation_id = operation_id.to_string();
        self.with_connection(|connection| {
            let tx = connection.transaction()?;
            let journal = load_journal_entry(&tx, &operation_id)?
                .ok_or_else(|| MetadataError::JournalNotFound(operation_id.clone()))?;
            let mut manifest = load_manifest_by_object_id(&tx, &journal.object_id)?
                .ok_or_else(|| MetadataError::ManifestNotFound(journal.object_id.clone()))?;

            if journal.state == "committed" && manifest.commit_state == CommitState::Committed {
                return Ok(manifest);
            }

            let committed_at = timestamp_now()?;
            let object_id = manifest.object_id.to_string();
            let bucket = manifest.bucket.clone();
            let object_key = manifest.key.clone();
            manifest.commit_state = CommitState::Committed;
            let manifest_json = serde_json::to_string(&manifest)?;

            tx.execute(
                r#"
                UPDATE object_manifests
                SET commit_state = 'committed',
                    manifest_json = ?2,
                    committed_at = ?3,
                    tombstoned_at = NULL
                WHERE object_id = ?1
                "#,
                params![object_id.clone(), manifest_json, committed_at],
            )?;
            tx.execute(
                r#"
                INSERT INTO active_objects (bucket, object_key, object_id, updated_at)
                VALUES (?1, ?2, ?3, ?4)
                ON CONFLICT(bucket, object_key) DO UPDATE SET
                    object_id = excluded.object_id,
                    updated_at = excluded.updated_at
                "#,
                params![bucket, object_key, object_id.clone(), committed_at],
            )?;
            tx.execute(
                "DELETE FROM recovery_markers WHERE marker_key = ?1",
                params![format!("staging:{}", journal.operation_id)],
            )?;
            tx.execute(
                r#"
                UPDATE operation_journal
                SET state = 'committed',
                    updated_at = ?2
                WHERE operation_id = ?1
                "#,
                params![operation_id, committed_at],
            )?;
            tx.commit()?;
            Ok(manifest)
        })
    }

    pub fn tombstone_manifest(
        &self,
        object_id: Uuid,
        reason: &str,
    ) -> Result<ObjectManifest, MetadataError> {
        let object_id = object_id.to_string();
        self.with_connection(|connection| {
            let tx = connection.transaction()?;
            let mut manifest = load_manifest_by_object_id(&tx, &object_id)?
                .ok_or_else(|| MetadataError::ManifestNotFound(object_id.clone()))?;

            let tombstoned_at = timestamp_now()?;
            let bucket = manifest.bucket.clone();
            let object_key = manifest.key.clone();
            let manifest_object_id = manifest.object_id.to_string();
            manifest.commit_state = CommitState::Tombstoned;
            let manifest_json = serde_json::to_string(&manifest)?;

            tx.execute(
                r#"
                UPDATE object_manifests
                SET commit_state = 'tombstoned',
                    manifest_json = ?2,
                    tombstoned_at = ?3
                WHERE object_id = ?1
                "#,
                params![object_id, manifest_json, tombstoned_at],
            )?;
            tx.execute(
                r#"
                DELETE FROM active_objects
                WHERE bucket = ?1 AND object_key = ?2 AND object_id = ?3
                "#,
                params![bucket, object_key, manifest_object_id.clone()],
            )?;
            tx.execute(
                "DELETE FROM recovery_markers WHERE object_id = ?1",
                params![manifest_object_id.clone()],
            )?;
            tx.execute(
                r#"
                UPDATE operation_journal
                SET state = 'tombstoned',
                    error = ?2,
                    updated_at = ?3
                WHERE object_id = ?1
                "#,
                params![manifest_object_id, reason, tombstoned_at],
            )?;
            tx.commit()?;
            Ok(manifest)
        })
    }

    pub fn get_manifest(&self, object_id: Uuid) -> Result<Option<ObjectManifest>, MetadataError> {
        let object_id = object_id.to_string();
        self.with_connection(|connection| load_manifest_by_object_id(connection, &object_id))
    }

    pub fn get_active_manifest(
        &self,
        bucket: &str,
        object_key: &str,
    ) -> Result<Option<ObjectManifest>, MetadataError> {
        self.with_connection(|connection| {
            let object_id: Option<String> = connection
                .query_row(
                    r#"
                    SELECT object_id
                    FROM active_objects
                    WHERE bucket = ?1 AND object_key = ?2
                    "#,
                    params![bucket, object_key],
                    |row| row.get(0),
                )
                .optional()?;
            match object_id {
                Some(object_id) => load_manifest_by_object_id(connection, &object_id),
                None => Ok(None),
            }
        })
    }

    pub fn rebuild_index(&self) -> Result<RebuildReport, MetadataError> {
        self.with_connection(rebuild_index_internal)
    }

    pub fn verify_index(&self) -> Result<VerifyReport, MetadataError> {
        self.with_connection(verify_index_internal)
    }

    pub fn startup_reconcile(&self) -> Result<RebuildReport, MetadataError> {
        self.rebuild_index()
    }

    pub fn list_manifests(&self) -> Result<Vec<ObjectManifest>, MetadataError> {
        self.with_connection(|connection| {
            let mut stmt = connection.prepare(
                r#"
                SELECT manifest_json
                FROM object_manifests
                ORDER BY created_at ASC, object_id ASC
                "#,
            )?;
            let mut manifests = Vec::new();
            for row in stmt.query_map([], |row| row.get::<_, String>(0))? {
                let json = row?;
                let manifest = serde_json::from_str::<ObjectManifest>(&json)?;
                manifest
                    .validate()
                    .map_err(MetadataError::InvalidManifest)?;
                manifests.push(manifest);
            }
            Ok(manifests)
        })
    }

    pub fn list_tombstoned_manifests(
        &self,
    ) -> Result<Vec<TombstonedManifestRecord>, MetadataError> {
        self.with_connection(|connection| {
            let mut stmt = connection.prepare(
                r#"
                SELECT manifest_json, tombstoned_at
                FROM object_manifests
                WHERE commit_state = 'tombstoned'
                ORDER BY tombstoned_at ASC, object_id ASC
                "#,
            )?;
            let mut manifests = Vec::new();
            for row in stmt.query_map([], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
            })? {
                let (json, tombstoned_at) = row?;
                let manifest = serde_json::from_str::<ObjectManifest>(&json)?;
                manifest
                    .validate()
                    .map_err(MetadataError::InvalidManifest)?;
                manifests.push(TombstonedManifestRecord {
                    manifest,
                    tombstoned_at: parse_rfc3339_timestamp(&tombstoned_at)?,
                });
            }
            Ok(manifests)
        })
    }

    pub fn list_journal_entries(&self) -> Result<Vec<JournalEntry>, MetadataError> {
        self.with_connection(|connection| {
            let mut stmt = connection.prepare(
                r#"
                SELECT operation_id, object_id, bucket, object_key, operation_kind, state, error
                FROM operation_journal
                ORDER BY created_at ASC, operation_id ASC
                "#,
            )?;
            let mut entries = Vec::new();
            for row in stmt.query_map([], |row| {
                Ok(JournalEntry {
                    operation_id: Uuid::parse_str(&row.get::<_, String>(0)?).map_err(|error| {
                        rusqlite::Error::FromSqlConversionFailure(
                            0,
                            rusqlite::types::Type::Text,
                            Box::new(error),
                        )
                    })?,
                    object_id: Uuid::parse_str(&row.get::<_, String>(1)?).map_err(|error| {
                        rusqlite::Error::FromSqlConversionFailure(
                            1,
                            rusqlite::types::Type::Text,
                            Box::new(error),
                        )
                    })?,
                    bucket: row.get(2)?,
                    object_key: row.get(3)?,
                    operation_kind: OperationKind::from_str(&row.get::<_, String>(4)?).map_err(
                        |error| {
                            rusqlite::Error::FromSqlConversionFailure(
                                4,
                                rusqlite::types::Type::Text,
                                Box::new(error),
                            )
                        },
                    )?,
                    state: row.get(5)?,
                    reason: row.get(6)?,
                })
            })? {
                entries.push(row?);
            }
            Ok(entries)
        })
    }

    pub fn create_multipart_session(
        &self,
        session: MultipartSession,
    ) -> Result<MultipartSession, MetadataError> {
        let upload_id = session.upload_id.to_string();
        let bucket = session.bucket.clone();
        let object_key = session.key.clone();
        let state = session.state.as_str().to_string();
        let created_at = timestamp_now()?;
        let updated_at = created_at.clone();
        let session_json = serde_json::to_string(&session)?;

        self.with_connection(|connection| {
            let tx = connection.transaction()?;
            tx.execute(
                r#"
                INSERT INTO multipart_uploads (
                    upload_id,
                    bucket,
                    object_key,
                    state,
                    session_json,
                    created_at,
                    updated_at
                )
                VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
                ON CONFLICT(upload_id) DO UPDATE SET
                    bucket = excluded.bucket,
                    object_key = excluded.object_key,
                    state = excluded.state,
                    session_json = excluded.session_json,
                    updated_at = excluded.updated_at
                "#,
                params![
                    upload_id,
                    bucket,
                    object_key,
                    state,
                    session_json,
                    created_at,
                    updated_at
                ],
            )?;
            tx.commit()?;
            Ok(session)
        })
    }

    pub fn get_multipart_session(
        &self,
        upload_id: Uuid,
    ) -> Result<Option<MultipartSession>, MetadataError> {
        let upload_id = upload_id.to_string();
        self.with_connection(|connection| {
            let json: Option<String> = connection
                .query_row(
                    "SELECT session_json FROM multipart_uploads WHERE upload_id = ?1",
                    params![upload_id],
                    |row| row.get(0),
                )
                .optional()?;
            match json {
                Some(json) => Ok(Some(serde_json::from_str::<MultipartSession>(&json)?)),
                None => Ok(None),
            }
        })
    }

    pub fn list_multipart_sessions(
        &self,
        bucket: Option<&str>,
        prefix: Option<&str>,
    ) -> Result<Vec<MultipartSession>, MetadataError> {
        self.with_connection(|connection| {
            let mut sql = String::from(
                r#"
                SELECT session_json
                FROM multipart_uploads
                WHERE state IN ('initiated', 'uploading', 'completing', 'recovery_required')
                "#,
            );
            if bucket.is_some() {
                sql.push_str(" AND bucket = ?1");
            }
            if prefix.is_some() {
                if bucket.is_some() {
                    sql.push_str(" AND object_key LIKE ?2 || '%'");
                } else {
                    sql.push_str(" AND object_key LIKE ?1 || '%'");
                }
            }
            sql.push_str(" ORDER BY object_key ASC, upload_id ASC");

            let mut stmt = connection.prepare(&sql)?;
            let mut sessions = Vec::new();
            match (bucket, prefix) {
                (Some(bucket), Some(prefix)) => {
                    let mut rows = stmt.query(params![bucket, prefix])?;
                    while let Some(row) = rows.next()? {
                        let json = row.get::<_, String>(0)?;
                        sessions.push(serde_json::from_str::<MultipartSession>(&json)?);
                    }
                }
                (Some(bucket), None) => {
                    let mut rows = stmt.query(params![bucket])?;
                    while let Some(row) = rows.next()? {
                        let json = row.get::<_, String>(0)?;
                        sessions.push(serde_json::from_str::<MultipartSession>(&json)?);
                    }
                }
                (None, Some(prefix)) => {
                    let mut rows = stmt.query(params![prefix])?;
                    while let Some(row) = rows.next()? {
                        let json = row.get::<_, String>(0)?;
                        sessions.push(serde_json::from_str::<MultipartSession>(&json)?);
                    }
                }
                (None, None) => {
                    let mut rows = stmt.query([])?;
                    while let Some(row) = rows.next()? {
                        let json = row.get::<_, String>(0)?;
                        sessions.push(serde_json::from_str::<MultipartSession>(&json)?);
                    }
                }
            }
            Ok(sessions)
        })
    }

    pub fn update_multipart_session_state(
        &self,
        upload_id: Uuid,
        state: MultipartState,
    ) -> Result<MultipartSession, MetadataError> {
        let upload_id_string = upload_id.to_string();
        self.with_connection(|connection| {
            let tx = connection.transaction()?;
            let json: Option<String> = tx
                .query_row(
                    "SELECT session_json FROM multipart_uploads WHERE upload_id = ?1",
                    params![upload_id_string.clone()],
                    |row| row.get(0),
                )
                .optional()?;
            let mut session = match json {
                Some(json) => serde_json::from_str::<MultipartSession>(&json)?,
                None => {
                    return Err(MetadataError::MultipartSessionNotFound(
                        upload_id_string.clone(),
                    ));
                }
            };
            session.state = state;
            session.updated_at = OffsetDateTime::now_utc();
            let session_json = serde_json::to_string(&session)?;
            let updated_at = timestamp_now()?;
            tx.execute(
                r#"
                UPDATE multipart_uploads
                SET state = ?2,
                    session_json = ?3,
                    updated_at = ?4
                WHERE upload_id = ?1
                "#,
                params![upload_id_string, state.as_str(), session_json, updated_at],
            )?;
            tx.commit()?;
            Ok(session)
        })
    }

    pub fn delete_multipart_session(&self, upload_id: Uuid) -> Result<(), MetadataError> {
        let upload_id = upload_id.to_string();
        self.with_connection(|connection| {
            let tx = connection.transaction()?;
            tx.execute(
                "DELETE FROM multipart_uploads WHERE upload_id = ?1",
                params![upload_id],
            )?;
            tx.commit()?;
            Ok(())
        })
    }

    pub fn put_multipart_part(&self, part: MultipartPart) -> Result<MultipartPart, MetadataError> {
        let upload_id = part.upload_id.to_string();
        let part_json = serde_json::to_string(&part)?;
        let created_at = timestamp_now()?;
        self.with_connection(|connection| {
            let tx = connection.transaction()?;
            tx.execute(
                r#"
                INSERT INTO multipart_parts (
                    upload_id,
                    part_number,
                    part_json,
                    created_at
                )
                VALUES (?1, ?2, ?3, ?4)
                ON CONFLICT(upload_id, part_number) DO UPDATE SET
                    part_json = excluded.part_json
                "#,
                params![upload_id, part.part_number, part_json, created_at],
            )?;
            tx.execute(
                r#"
                UPDATE multipart_uploads
                SET state = CASE
                        WHEN state = 'initiated' THEN 'uploading'
                        ELSE state
                    END,
                    updated_at = ?2
                WHERE upload_id = ?1
                "#,
                params![part.upload_id.to_string(), timestamp_now()?],
            )?;
            tx.commit()?;
            Ok(part)
        })
    }

    pub fn list_multipart_parts(
        &self,
        upload_id: Uuid,
    ) -> Result<Vec<MultipartPart>, MetadataError> {
        let upload_id = upload_id.to_string();
        self.with_connection(|connection| {
            let mut stmt = connection.prepare(
                r#"
                SELECT part_json
                FROM multipart_parts
                WHERE upload_id = ?1
                ORDER BY part_number ASC
                "#,
            )?;
            let mut parts = Vec::new();
            let mut rows = stmt.query(params![upload_id])?;
            while let Some(row) = rows.next()? {
                let json = row.get::<_, String>(0)?;
                parts.push(serde_json::from_str::<MultipartPart>(&json)?);
            }
            Ok(parts)
        })
    }

    pub fn get_multipart_part(
        &self,
        upload_id: Uuid,
        part_number: u32,
    ) -> Result<Option<MultipartPart>, MetadataError> {
        let upload_id = upload_id.to_string();
        self.with_connection(|connection| {
            let json: Option<String> = connection
                .query_row(
                    "SELECT part_json FROM multipart_parts WHERE upload_id = ?1 AND part_number = ?2",
                    params![upload_id, part_number],
                    |row| row.get(0),
                )
                .optional()?;
            match json {
                Some(json) => Ok(Some(serde_json::from_str::<MultipartPart>(&json)?)),
                None => Ok(None),
            }
        })
    }

    pub fn delete_multipart_parts(&self, upload_id: Uuid) -> Result<(), MetadataError> {
        let upload_id = upload_id.to_string();
        self.with_connection(|connection| {
            let tx = connection.transaction()?;
            tx.execute(
                "DELETE FROM multipart_parts WHERE upload_id = ?1",
                params![upload_id],
            )?;
            tx.commit()?;
            Ok(())
        })
    }

    pub fn update_manifest_state(
        &self,
        object_id: Uuid,
        commit_state: CommitState,
    ) -> Result<ObjectManifest, MetadataError> {
        let object_id = object_id.to_string();
        self.with_connection(|connection| {
            let tx = connection.transaction()?;
            let mut manifest = load_manifest_by_object_id(&tx, &object_id)?
                .ok_or_else(|| MetadataError::ManifestNotFound(object_id.clone()))?;
            manifest.commit_state = commit_state;
            let manifest_json = serde_json::to_string(&manifest)?;

            tx.execute(
                r#"
                UPDATE object_manifests
                SET commit_state = ?2,
                    manifest_json = ?3
                WHERE object_id = ?1
                "#,
                params![object_id.clone(), commit_state.as_str(), manifest_json],
            )?;

            match commit_state {
                CommitState::Committed => {
                    tx.execute(
                        r#"
                        INSERT INTO active_objects (bucket, object_key, object_id, updated_at)
                        VALUES (?1, ?2, ?3, ?4)
                        ON CONFLICT(bucket, object_key) DO UPDATE SET
                            object_id = excluded.object_id,
                            updated_at = excluded.updated_at
                        "#,
                        params![
                            manifest.bucket.clone(),
                            manifest.key.clone(),
                            object_id.clone(),
                            timestamp_now()?
                        ],
                    )?;
                }
                CommitState::Tombstoned
                | CommitState::Orphaned
                | CommitState::RecoveryRequired
                | CommitState::Staging => {
                    tx.execute(
                        "DELETE FROM active_objects WHERE object_id = ?1",
                        params![object_id.clone()],
                    )?;
                }
            }

            tx.commit()?;
            Ok(manifest)
        })
    }

    fn with_connection<T>(
        &self,
        f: impl FnOnce(&mut Connection) -> Result<T, MetadataError>,
    ) -> Result<T, MetadataError> {
        let mut connection = self
            .connection
            .lock()
            .map_err(|_| MetadataError::Poisoned)?;
        f(&mut connection)
    }

    fn ensure_recovery_marker(&self, details: &JournalDetails) -> Result<(), MetadataError> {
        self.with_connection(|connection| {
            let now = timestamp_now()?;
            connection.execute(
                r#"
                INSERT INTO recovery_markers (
                    marker_key,
                    object_id,
                    bucket,
                    object_key,
                    marker_state,
                    details_json,
                    created_at,
                    updated_at
                )
                VALUES (?1, ?2, ?3, ?4, 'staging', ?5, ?6, ?6)
                ON CONFLICT(marker_key) DO UPDATE SET
                    object_id = excluded.object_id,
                    bucket = excluded.bucket,
                    object_key = excluded.object_key,
                    marker_state = excluded.marker_state,
                    details_json = excluded.details_json,
                    updated_at = excluded.updated_at
                "#,
                params![
                    format!("staging:{}", details.operation_id),
                    details.object_id,
                    details.bucket,
                    details.object_key,
                    serde_json::to_string(details)?,
                    now,
                ],
            )?;
            Ok(())
        })
    }
}

// ---- Auth / operator-account storage (phase 9). Rows live in the same single-writer
// metadata.sqlite so a backup/restore of the store already covers user + session state. ----

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DbUser {
    pub id: String,
    pub username: String,
    pub password_hash: String,
    pub role: String,
    pub display_name: String,
    pub disabled: bool,
    pub token_version: i64,
    pub created_at: OffsetDateTime,
    pub updated_at: OffsetDateTime,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DbSession {
    pub cookie_id: String,
    pub user_id: String,
    pub token_version: i64,
    pub issued_at: OffsetDateTime,
    pub expires_at: OffsetDateTime,
    pub created_ip: Option<String>,
    pub revoked_at: Option<OffsetDateTime>,
}

fn row_user(
    connection: &Connection,
    sql: &str,
    params: &[&dyn rusqlite::ToSql],
) -> Result<Option<DbUser>, MetadataError> {
    let mut stmt = connection.prepare(sql)?;
    let mut rows = stmt.query(params)?;
    let row = match rows.next()? {
        Some(row) => row,
        None => return Ok(None),
    };
    Ok(Some(read_user_row(row)?))
}

fn read_user_row(row: &rusqlite::Row<'_>) -> Result<DbUser, MetadataError> {
    Ok(DbUser {
        id: row.get(0)?,
        username: row.get(1)?,
        password_hash: row.get(2)?,
        role: row.get(3)?,
        display_name: row.get(4)?,
        disabled: row.get::<_, i64>(5)? != 0,
        token_version: row.get(6)?,
        created_at: parse_timestamp(&row.get::<_, String>(7)?, 7)?,
        updated_at: parse_timestamp(&row.get::<_, String>(8)?, 8)?,
    })
}

fn parse_timestamp(value: &str, index: usize) -> Result<OffsetDateTime, MetadataError> {
    OffsetDateTime::parse(value, &Rfc3339)
        .map_err(|error| {
            rusqlite::Error::FromSqlConversionFailure(
                index,
                rusqlite::types::Type::Text,
                Box::new(error),
            )
        })
        .map_err(MetadataError::Sqlite)
}

impl MetadataStore {
    pub fn create_user(
        &self,
        id: &str,
        username: &str,
        password_hash: &str,
        role: &str,
        display_name: &str,
    ) -> Result<DbUser, MetadataError> {
        let now = timestamp_now()?;
        self.with_connection(|connection| {
            connection.execute(
                r#"
                INSERT INTO users (
                    id, username, password_hash, role, display_name,
                    disabled, token_version, created_at, updated_at
                )
                VALUES (?1, ?2, ?3, ?4, ?5, 0, 0, ?6, ?6)
                "#,
                params![id, username, password_hash, role, display_name, now],
            )?;
            Ok(())
        })?;
        self.get_user(username)?
            .ok_or_else(|| MetadataError::InvalidManifest("created user not readable".to_string()))
    }

    pub fn get_user(&self, username: &str) -> Result<Option<DbUser>, MetadataError> {
        self.with_connection(|connection| {
            row_user(
                connection,
                "SELECT id, username, password_hash, role, display_name, disabled, token_version, created_at, updated_at
                 FROM users WHERE username = ?1",
                &[&username],
            )
        })
    }

    pub fn get_user_by_id(&self, id: &str) -> Result<Option<DbUser>, MetadataError> {
        self.with_connection(|connection| {
            row_user(
                connection,
                "SELECT id, username, password_hash, role, display_name, disabled, token_version, created_at, updated_at
                 FROM users WHERE id = ?1",
                &[&id],
            )
        })
    }

    pub fn list_users(&self) -> Result<Vec<DbUser>, MetadataError> {
        self.with_connection(|connection| {
            let mut stmt = connection.prepare(
                "SELECT id, username, password_hash, role, display_name, disabled, token_version, created_at, updated_at
                 FROM users ORDER BY username ASC",
            )?;
            let mut rows = stmt.query([])?;
            let mut out = Vec::new();
            while let Some(row) = rows.next()? {
                out.push(read_user_row(row)?);
            }
            Ok(out)
        })
    }

    pub fn user_count(&self) -> Result<u64, MetadataError> {
        self.with_connection(|conn| {
            Ok(conn.query_row("SELECT COUNT(*) FROM users", [], |r| r.get::<_, u64>(0))?)
        })
    }

    pub fn delete_user(&self, id: &str) -> Result<(), MetadataError> {
        self.with_connection(|connection| {
            connection.execute("DELETE FROM users WHERE id = ?1", params![id])?;
            Ok(())
        })
    }

    pub fn set_user_password(&self, id: &str, password_hash: &str) -> Result<(), MetadataError> {
        let now = timestamp_now()?;
        self.with_connection(|connection| {
            connection.execute(
                r#"
                UPDATE users
                SET password_hash = ?2, token_version = token_version + 1, updated_at = ?3
                WHERE id = ?1
                "#,
                params![id, password_hash, now],
            )?;
            Ok(())
        })
    }

    pub fn set_user_disabled(&self, id: &str, disabled: bool) -> Result<(), MetadataError> {
        let now = timestamp_now()?;
        self.with_connection(|connection| {
            connection.execute(
                "UPDATE users SET disabled = ?2, updated_at = ?3 WHERE id = ?1",
                params![id, disabled, now],
            )?;
            Ok(())
        })
    }

    pub fn current_token_version(&self, id: &str) -> Result<i64, MetadataError> {
        self.with_connection(|connection| {
            let version = connection
                .query_row(
                    "SELECT token_version FROM users WHERE id = ?1",
                    params![id],
                    |row| row.get::<_, i64>(0),
                )
                .optional()?
                .unwrap_or(0);
            Ok(version)
        })
    }

    pub fn insert_session(&self, session: &DbSession) -> Result<(), MetadataError> {
        let (issued_at, expires_at) = (session.issued_at, session.expires_at);
        self.with_connection(|connection| {
            connection.execute(
                r#"
                INSERT INTO admin_sessions (
                    cookie_id, user_id, token_version, issued_at, expires_at, created_ip, revoked_at
                )
                VALUES (?1, ?2, ?3, ?4, ?5, ?6, NULL)
                "#,
                params![
                    session.cookie_id,
                    session.user_id,
                    session.token_version,
                    issued_at
                        .format(&Rfc3339)
                        .map_err(MetadataError::TimeFormat)?,
                    expires_at
                        .format(&Rfc3339)
                        .map_err(MetadataError::TimeFormat)?,
                    session.created_ip,
                ],
            )?;
            Ok(())
        })
    }

    pub fn get_session(&self, cookie_id: &str) -> Result<Option<DbSession>, MetadataError> {
        self.with_connection(|connection| {
            let mut stmt = connection.prepare(
                r#"
                SELECT cookie_id, user_id, token_version, issued_at, expires_at, created_ip, revoked_at
                FROM admin_sessions WHERE cookie_id = ?1
                "#,
            )?;
            let mut rows = stmt.query(params![cookie_id])?;
            let row = match rows.next()? {
                Some(row) => row,
                None => return Ok(None),
            };
            let revoked_at: Option<String> = row.get(6)?;
            Ok(Some(DbSession {
                cookie_id: row.get(0)?,
                user_id: row.get(1)?,
                token_version: row.get(2)?,
                issued_at: parse_timestamp(&row.get::<_, String>(3)?, 3)?,
                expires_at: parse_timestamp(&row.get::<_, String>(4)?, 4)?,
                created_ip: row.get(5)?,
                revoked_at: match revoked_at {
                    Some(value) => Some(parse_timestamp(&value, 6)?),
                    None => None,
                },
            }))
        })
    }

    pub fn revoke_session(&self, cookie_id: &str) -> Result<(), MetadataError> {
        let now = timestamp_now()?;
        self.with_connection(|connection| {
            connection.execute(
                "UPDATE admin_sessions SET revoked_at = ?2 WHERE cookie_id = ?1 AND revoked_at IS NULL",
                params![cookie_id, now],
            )?;
            Ok(())
        })
    }

    pub fn revoke_user_sessions(&self, user_id: &str) -> Result<(), MetadataError> {
        let now = timestamp_now()?;
        self.with_connection(|connection| {
            connection.execute(
                "UPDATE admin_sessions SET revoked_at = ?2 WHERE user_id = ?1 AND revoked_at IS NULL",
                params![user_id, now],
            )?;
            Ok(())
        })
    }

    pub fn sweep_sessions(&self, cutoff: OffsetDateTime) -> Result<u64, MetadataError> {
        self.with_connection(|connection| {
            let n = connection.execute(
                "DELETE FROM admin_sessions WHERE revoked_at IS NOT NULL AND revoked_at < ?1",
                params![cutoff.format(&Rfc3339).map_err(MetadataError::TimeFormat)?],
            )?;
            Ok(n as u64)
        })
    }
}

fn apply_migrations(connection: &mut Connection) -> Result<(), MetadataError> {
    let current_version = read_schema_version(connection)?;
    if current_version > SCHEMA_VERSION {
        return Err(MetadataError::UnsupportedSchemaVersion(current_version));
    }
    if current_version >= SCHEMA_VERSION {
        return Ok(());
    }

    let tx = connection.transaction()?;
    tx.execute_batch(
        r#"
        CREATE TABLE IF NOT EXISTS schema_version (
            id INTEGER PRIMARY KEY CHECK (id = 1),
            version INTEGER NOT NULL,
            applied_at TEXT NOT NULL
        );

        CREATE TABLE IF NOT EXISTS buckets (
            name TEXT PRIMARY KEY,
            created_at TEXT NOT NULL,
            deleted_at TEXT,
            versioning_enabled INTEGER NOT NULL DEFAULT 0,
            object_locking_enabled INTEGER NOT NULL DEFAULT 0
        );

        CREATE INDEX IF NOT EXISTS idx_buckets_deleted_at
            ON buckets(deleted_at);

        CREATE TABLE IF NOT EXISTS operation_journal (
            operation_id TEXT PRIMARY KEY,
            object_id TEXT NOT NULL,
            bucket TEXT NOT NULL,
            object_key TEXT NOT NULL,
            operation_kind TEXT NOT NULL,
            state TEXT NOT NULL,
            manifest_json TEXT,
            error TEXT,
            created_at TEXT NOT NULL,
            updated_at TEXT NOT NULL
        );

        CREATE INDEX IF NOT EXISTS idx_operation_journal_bucket_key
            ON operation_journal(bucket, object_key);

        CREATE INDEX IF NOT EXISTS idx_operation_journal_state
            ON operation_journal(state);

        CREATE TABLE IF NOT EXISTS object_manifests (
            object_id TEXT PRIMARY KEY,
            bucket TEXT NOT NULL,
            object_key TEXT NOT NULL,
            version_id TEXT,
            commit_state TEXT NOT NULL,
            manifest_json TEXT NOT NULL,
            created_at TEXT NOT NULL,
            committed_at TEXT,
            tombstoned_at TEXT
        );

        CREATE INDEX IF NOT EXISTS idx_object_manifests_bucket_key
            ON object_manifests(bucket, object_key);

        CREATE INDEX IF NOT EXISTS idx_object_manifests_state
            ON object_manifests(commit_state);

        CREATE TABLE IF NOT EXISTS active_objects (
            bucket TEXT NOT NULL,
            object_key TEXT NOT NULL,
            object_id TEXT NOT NULL,
            updated_at TEXT NOT NULL,
            PRIMARY KEY(bucket, object_key),
            FOREIGN KEY(object_id) REFERENCES object_manifests(object_id) ON DELETE CASCADE
        );

        CREATE TABLE IF NOT EXISTS recovery_markers (
            marker_key TEXT PRIMARY KEY,
            object_id TEXT NOT NULL,
            bucket TEXT NOT NULL,
            object_key TEXT NOT NULL,
            marker_state TEXT NOT NULL,
            details_json TEXT NOT NULL,
            created_at TEXT NOT NULL,
            updated_at TEXT NOT NULL
        );

        CREATE INDEX IF NOT EXISTS idx_recovery_markers_state
            ON recovery_markers(marker_state);

        CREATE TABLE IF NOT EXISTS multipart_uploads (
            upload_id TEXT PRIMARY KEY,
            bucket TEXT NOT NULL,
            object_key TEXT NOT NULL,
            state TEXT NOT NULL,
            session_json TEXT NOT NULL,
            created_at TEXT NOT NULL,
            updated_at TEXT NOT NULL
        );

        CREATE INDEX IF NOT EXISTS idx_multipart_uploads_bucket_key
            ON multipart_uploads(bucket, object_key);

        CREATE INDEX IF NOT EXISTS idx_multipart_uploads_state
            ON multipart_uploads(state);

        CREATE TABLE IF NOT EXISTS multipart_parts (
            upload_id TEXT NOT NULL,
            part_number INTEGER NOT NULL,
            part_json TEXT NOT NULL,
            created_at TEXT NOT NULL,
            PRIMARY KEY(upload_id, part_number),
            FOREIGN KEY(upload_id) REFERENCES multipart_uploads(upload_id) ON DELETE CASCADE
        );

        CREATE INDEX IF NOT EXISTS idx_multipart_parts_upload
            ON multipart_parts(upload_id, part_number);

        CREATE TABLE IF NOT EXISTS users (
            id TEXT PRIMARY KEY,
            username TEXT NOT NULL UNIQUE,
            password_hash TEXT NOT NULL,
            role TEXT NOT NULL DEFAULT 'admin',
            display_name TEXT NOT NULL DEFAULT '',
            disabled INTEGER NOT NULL DEFAULT 0,
            token_version INTEGER NOT NULL DEFAULT 0,
            created_at TEXT NOT NULL,
            updated_at TEXT NOT NULL
        );

        CREATE INDEX IF NOT EXISTS idx_users_username ON users(username);

        CREATE TABLE IF NOT EXISTS admin_sessions (
            cookie_id TEXT PRIMARY KEY,
            user_id TEXT NOT NULL,
            token_version INTEGER NOT NULL,
            issued_at TEXT NOT NULL,
            expires_at TEXT NOT NULL,
            created_ip TEXT,
            revoked_at TEXT,
            FOREIGN KEY(user_id) REFERENCES users(id) ON DELETE CASCADE
        );

        CREATE INDEX IF NOT EXISTS idx_admin_sessions_user ON admin_sessions(user_id);
        "#,
    )?;
    tx.execute(
        r#"
        INSERT INTO schema_version (id, version, applied_at)
        VALUES (1, ?1, ?2)
        ON CONFLICT(id) DO UPDATE SET
            version = excluded.version,
            applied_at = excluded.applied_at
        "#,
        params![SCHEMA_VERSION, timestamp_now()?],
    )?;
    tx.commit()?;
    Ok(())
}

fn rebuild_index_internal(connection: &mut Connection) -> Result<RebuildReport, MetadataError> {
    let tx = connection.transaction()?;
    tx.execute("DELETE FROM active_objects", [])?;

    let mut committed_rows = 0_u64;
    let mut staged_rows = 0_u64;
    let mut recovery_markers = 0_u64;

    {
        let mut stmt = tx.prepare(
            r#"
            SELECT operation_id, object_id, bucket, object_key, operation_kind, state
            FROM operation_journal
            ORDER BY created_at ASC, operation_id ASC
            "#,
        )?;
        let entries = stmt.query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, String>(4)?,
                row.get::<_, String>(5)?,
            ))
        })?;
        for entry in entries {
            let (operation_id, object_id, bucket, object_key, operation_kind, state) = entry?;
            if state != "staging" {
                continue;
            }
            staged_rows += 1;
            recovery_markers += 1;
            let details = JournalDetails {
                operation_id: operation_id.clone(),
                object_id,
                bucket,
                object_key,
                operation_kind: OperationKind::from_str(&operation_kind)?,
                state,
                reason: Some("pending startup reconciliation".to_string()),
            };
            tx.execute(
                r#"
                INSERT INTO recovery_markers (
                    marker_key,
                    object_id,
                    bucket,
                    object_key,
                    marker_state,
                    details_json,
                    created_at,
                    updated_at
                )
                VALUES (?1, ?2, ?3, ?4, 'staging', ?5, ?6, ?6)
                ON CONFLICT(marker_key) DO UPDATE SET
                    object_id = excluded.object_id,
                    bucket = excluded.bucket,
                    object_key = excluded.object_key,
                    marker_state = excluded.marker_state,
                    details_json = excluded.details_json,
                    updated_at = excluded.updated_at
                "#,
                params![
                    format!("staging:{}", operation_id),
                    details.object_id,
                    details.bucket,
                    details.object_key,
                    serde_json::to_string(&details)?,
                    timestamp_now()?,
                ],
            )?;
        }
    }

    {
        let mut stmt = tx.prepare(
            r#"
            SELECT bucket, object_key, object_id
            FROM object_manifests
            WHERE commit_state = 'committed'
            ORDER BY committed_at ASC, object_id ASC
            "#,
        )?;
        let entries = stmt.query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
            ))
        })?;
        for entry in entries {
            let (bucket, object_key, object_id) = entry?;
            committed_rows += 1;
            tx.execute(
                r#"
                INSERT INTO active_objects (bucket, object_key, object_id, updated_at)
                VALUES (?1, ?2, ?3, ?4)
                ON CONFLICT(bucket, object_key) DO UPDATE SET
                    object_id = excluded.object_id,
                    updated_at = excluded.updated_at
                "#,
                params![bucket, object_key, object_id, timestamp_now()?],
            )?;
        }
    }

    let active_rows = count_rows_tx(&tx, "SELECT COUNT(*) FROM active_objects")?;
    tx.commit()?;

    Ok(RebuildReport {
        committed_rows,
        active_rows,
        staged_rows,
        recovery_markers,
    })
}

fn verify_index_internal(connection: &mut Connection) -> Result<VerifyReport, MetadataError> {
    let mut expected = HashMap::<(String, String), String>::new();
    let mut staged_rows = 0_u64;

    {
        let mut stmt = connection.prepare(
            r#"
            SELECT bucket, object_key, object_id, commit_state
            FROM object_manifests
            ORDER BY committed_at ASC, object_id ASC
            "#,
        )?;
        let entries = stmt.query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
            ))
        })?;
        for entry in entries {
            let (bucket, object_key, object_id, commit_state) = entry?;
            if commit_state == "staging" {
                staged_rows += 1;
                continue;
            }
            if commit_state == "committed" {
                expected.insert((bucket, object_key), object_id);
            }
        }
    }

    let mut actual = HashMap::<(String, String), String>::new();
    {
        let mut stmt =
            connection.prepare("SELECT bucket, object_key, object_id FROM active_objects")?;
        let entries = stmt.query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
            ))
        })?;
        for entry in entries {
            let (bucket, object_key, object_id) = entry?;
            actual.insert((bucket, object_key), object_id);
        }
    }

    let mut mismatched_rows = 0_u64;
    for (key, expected_object_id) in &expected {
        match actual.get(key) {
            Some(actual_object_id) if actual_object_id == expected_object_id => {}
            _ => mismatched_rows += 1,
        }
    }
    for key in actual.keys() {
        if !expected.contains_key(key) {
            mismatched_rows += 1;
        }
    }

    Ok(VerifyReport {
        expected_active_rows: expected.len() as u64,
        actual_active_rows: actual.len() as u64,
        mismatched_rows,
        staged_rows,
    })
}

fn load_manifest_by_object_id<T>(
    connection: &T,
    object_id: &str,
) -> Result<Option<ObjectManifest>, MetadataError>
where
    T: QueryRowExt,
{
    let manifest_json: Option<String> = connection
        .query_row(
            "SELECT manifest_json FROM object_manifests WHERE object_id = ?1",
            params![object_id],
            |row| row.get(0),
        )
        .optional()?;
    match manifest_json {
        Some(json) => {
            let manifest = serde_json::from_str::<ObjectManifest>(&json)?;
            manifest
                .validate()
                .map_err(MetadataError::InvalidManifest)?;
            Ok(Some(manifest))
        }
        None => Ok(None),
    }
}

fn load_journal_entry<T>(
    connection: &T,
    operation_id: &str,
) -> Result<Option<JournalDetails>, MetadataError>
where
    T: QueryRowExt,
{
    let journal_json: Option<String> = connection
        .query_row(
            "SELECT manifest_json FROM operation_journal WHERE operation_id = ?1",
            params![operation_id],
            |row| row.get(0),
        )
        .optional()?;
    match journal_json {
        Some(json) => Ok(Some(serde_json::from_str::<JournalDetails>(&json)?)),
        None => Ok(None),
    }
}

fn load_bucket_record<T>(
    connection: &T,
    bucket: &str,
    include_deleted: bool,
) -> Result<Option<BucketRecord>, MetadataError>
where
    T: QueryRowExt,
{
    let mut sql = String::from(
        r#"
        SELECT name, created_at, deleted_at, versioning_enabled, object_locking_enabled
        FROM buckets
        WHERE name = ?1
        "#,
    );
    if !include_deleted {
        sql.push_str(" AND deleted_at IS NULL");
    }
    let record = connection
        .query_row(&sql, params![bucket], |row| {
            let created_at = row.get::<_, String>(1)?;
            let deleted_at = row.get::<_, Option<String>>(2)?;
            Ok(BucketRecord {
                name: row.get(0)?,
                created_at: OffsetDateTime::parse(&created_at, &Rfc3339).map_err(|error| {
                    rusqlite::Error::FromSqlConversionFailure(
                        1,
                        rusqlite::types::Type::Text,
                        Box::new(error),
                    )
                })?,
                deleted_at: match deleted_at {
                    Some(value) => {
                        Some(OffsetDateTime::parse(&value, &Rfc3339).map_err(|error| {
                            rusqlite::Error::FromSqlConversionFailure(
                                2,
                                rusqlite::types::Type::Text,
                                Box::new(error),
                            )
                        })?)
                    }
                    None => None,
                },
                versioning_enabled: row.get::<_, i64>(3)? != 0,
                object_locking_enabled: row.get::<_, i64>(4)? != 0,
            })
        })
        .optional()?;
    Ok(record)
}

fn manifest_json_with_state(
    manifest: &ObjectManifest,
    commit_state: CommitState,
) -> Result<String, MetadataError> {
    let mut manifest = manifest.clone();
    manifest.commit_state = commit_state;
    Ok(serde_json::to_string(&manifest)?)
}

fn create_private_metadata_file(path: &Path) -> Result<(), MetadataError> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        let _ = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .mode(0o600)
            .open(path)?;
        let mut permissions = std::fs::metadata(path)?.permissions();
        permissions.set_mode(0o600);
        std::fs::set_permissions(path, permissions)?;
    }
    #[cfg(not(unix))]
    {
        let _ = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)?;
    }
    Ok(())
}

fn read_schema_version(connection: &Connection) -> Result<u32, MetadataError> {
    if !table_exists(connection, "schema_version")? {
        return Ok(0);
    }
    let version = connection
        .query_row(
            "SELECT version FROM schema_version WHERE id = 1",
            [],
            |row| row.get::<_, u32>(0),
        )
        .optional()?
        .unwrap_or(0);
    Ok(version)
}

fn table_exists(connection: &Connection, name: &str) -> Result<bool, MetadataError> {
    let exists = connection
        .query_row(
            "SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = ?1",
            params![name],
            |row| row.get::<_, i64>(0),
        )
        .optional()?
        .is_some();
    Ok(exists)
}

fn timestamp_now() -> Result<String, MetadataError> {
    Ok(OffsetDateTime::now_utc().format(&Rfc3339)?)
}

fn parse_rfc3339_timestamp(value: &str) -> Result<OffsetDateTime, MetadataError> {
    OffsetDateTime::parse(value, &Rfc3339).map_err(|error| {
        MetadataError::InvalidManifest(format!("invalid timestamp {value}: {error}"))
    })
}

fn count_rows(connection: &Connection, sql: &str) -> Result<u64, MetadataError> {
    Ok(connection.query_row(sql, [], |row| row.get::<_, u64>(0))?)
}

fn count_rows_tx(connection: &rusqlite::Transaction<'_>, sql: &str) -> Result<u64, MetadataError> {
    Ok(connection.query_row(sql, [], |row| row.get::<_, u64>(0))?)
}

trait QueryRowExt {
    fn query_row<T, P, F>(&self, sql: &str, params: P, f: F) -> Result<T, rusqlite::Error>
    where
        P: rusqlite::Params,
        F: FnOnce(&rusqlite::Row<'_>) -> Result<T, rusqlite::Error>;
}

impl QueryRowExt for Connection {
    fn query_row<T, P, F>(&self, sql: &str, params: P, f: F) -> Result<T, rusqlite::Error>
    where
        P: rusqlite::Params,
        F: FnOnce(&rusqlite::Row<'_>) -> Result<T, rusqlite::Error>,
    {
        let mut statement = self.prepare(sql)?;
        statement.query_row(params, f)
    }
}

impl<'conn> QueryRowExt for rusqlite::Transaction<'conn> {
    fn query_row<T, P, F>(&self, sql: &str, params: P, f: F) -> Result<T, rusqlite::Error>
    where
        P: rusqlite::Params,
        F: FnOnce(&rusqlite::Row<'_>) -> Result<T, rusqlite::Error>,
    {
        let mut statement = self.prepare(sql)?;
        statement.query_row(params, f)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::manifest::CommittedManifestArgs;
    use tempfile::tempdir;

    fn sample_manifest(bucket: &str, key: &str) -> ObjectManifest {
        ObjectManifest::committed(CommittedManifestArgs {
            bucket: bucket.to_string(),
            key: key.to_string(),
            content_length: 11,
            content_type: "text/plain".to_string(),
            checksum_algorithm: "sha256".to_string(),
            whole_object: "abcd".to_string(),
            peer_id: "peer".to_string(),
            message_id: 99,
        })
    }

    #[test]
    fn open_in_memory_applies_schema() {
        let store = MetadataStore::open_in_memory().expect("open");
        assert_eq!(store.schema_version().expect("schema"), SCHEMA_VERSION);
        let status = store.status().expect("status");
        assert_eq!(status.committed_objects, 0);
        assert_eq!(status.active_objects, 0);
        assert_eq!(status.staged_objects, 0);
    }

    #[test]
    fn stage_commit_and_read_visible_object() {
        let store = MetadataStore::open_in_memory().expect("open");
        let manifest = sample_manifest("bucket", "key.txt");
        let operation_id = store
            .stage_manifest(OperationKind::Put, manifest.clone())
            .expect("stage");
        let committed = store.commit_manifest(operation_id).expect("commit");
        assert_eq!(committed.commit_state, CommitState::Committed);

        let fetched = store
            .get_active_manifest("bucket", "key.txt")
            .expect("fetch")
            .expect("visible");
        assert_eq!(fetched.object_id, manifest.object_id);
        assert_eq!(fetched.key, "key.txt");
        assert_eq!(store.verify_index().expect("verify").mismatched_rows, 0);
    }

    #[test]
    fn overwrite_keeps_latest_pointer_only() {
        let store = MetadataStore::open_in_memory().expect("open");
        let first = sample_manifest("bucket", "key.txt");
        let first_op = store
            .stage_manifest(OperationKind::Put, first.clone())
            .expect("stage first");
        store.commit_manifest(first_op).expect("commit first");

        let second = sample_manifest("bucket", "key.txt");
        let second_op = store
            .stage_manifest(OperationKind::Put, second.clone())
            .expect("stage second");
        store.commit_manifest(second_op).expect("commit second");

        let active = store
            .get_active_manifest("bucket", "key.txt")
            .expect("fetch")
            .expect("visible");
        assert_eq!(active.object_id, second.object_id);
        assert_ne!(active.object_id, first.object_id);
        assert_eq!(store.verify_index().expect("verify").mismatched_rows, 0);
    }

    #[test]
    fn tombstone_removes_active_pointer_but_keeps_history() {
        let store = MetadataStore::open_in_memory().expect("open");
        let manifest = sample_manifest("bucket", "key.txt");
        let operation_id = store
            .stage_manifest(OperationKind::Put, manifest.clone())
            .expect("stage");
        store.commit_manifest(operation_id).expect("commit");

        let tombstoned = store
            .tombstone_manifest(manifest.object_id, "operator delete")
            .expect("tombstone");
        assert_eq!(tombstoned.commit_state, CommitState::Tombstoned);
        assert!(
            store
                .get_active_manifest("bucket", "key.txt")
                .expect("fetch")
                .is_none()
        );
        assert!(
            store
                .get_manifest(manifest.object_id)
                .expect("fetch history")
                .is_some()
        );
    }

    #[test]
    fn rebuild_restores_missing_active_rows() {
        let dir = tempdir().expect("tempdir");
        let path = dir.path().join("metadata.sqlite");
        let store = MetadataStore::open(&path).expect("open");
        let manifest = sample_manifest("bucket", "key.txt");
        let operation_id = store
            .stage_manifest(OperationKind::Put, manifest.clone())
            .expect("stage");
        store.commit_manifest(operation_id).expect("commit");

        store
            .with_connection(|connection| {
                connection.execute("DELETE FROM active_objects", [])?;
                Ok(())
            })
            .expect("corrupt");

        let report = store.rebuild_index().expect("rebuild");
        assert_eq!(report.active_rows, 1);
        assert_eq!(store.verify_index().expect("verify").mismatched_rows, 0);
        let active = store
            .get_active_manifest("bucket", "key.txt")
            .expect("fetch")
            .expect("visible");
        assert_eq!(active.object_id, manifest.object_id);
    }

    #[test]
    fn migration_from_version_zero_upgrades_cleanly() {
        let dir = tempdir().expect("tempdir");
        let path = dir.path().join("metadata.sqlite");
        {
            let connection = Connection::open(&path).expect("open");
            connection
                .execute_batch(
                    r#"
                    CREATE TABLE schema_version (
                        id INTEGER PRIMARY KEY CHECK (id = 1),
                        version INTEGER NOT NULL,
                        applied_at TEXT NOT NULL
                    );
                    INSERT INTO schema_version (id, version, applied_at)
                    VALUES (1, 0, '2026-08-29T00:00:00Z');
                    "#,
                )
                .expect("seed");
        }

        let store = MetadataStore::open(&path).expect("open");
        assert_eq!(store.schema_version().expect("schema"), SCHEMA_VERSION);
        assert_eq!(store.status().expect("status").active_objects, 0);
    }

    #[test]
    fn users_roundtrip_and_case_preservation() {
        let store = MetadataStore::open_in_memory().expect("open");
        let user = store
            .create_user(
                "u1",
                "admin",
                "$argon2id$v=19$m=65536,t=3,p=4$c2FsdHNhbHRzYWx0cw$hashhashhashhashhashhashhashhashhashhash",
                "superadmin",
                "Op One",
            )
            .expect("create");
        assert_eq!(user.role, "superadmin");
        let fetched = store.get_user("admin").expect("get").expect("exists");
        assert_eq!(fetched.id, "u1");
        assert!(!fetched.password_hash.is_empty());
        assert_eq!(store.user_count().expect("count"), 1);

        let listed = store.list_users().expect("list");
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].username, "admin");

        let missing = store.get_user("nobody").expect("get");
        assert!(missing.is_none());
    }

    #[test]
    fn password_change_bumps_token_version_and_revokes() {
        let store = MetadataStore::open_in_memory().expect("open");
        let user = store
            .create_user("u1", "bob", "old-hash-line", "admin", "")
            .expect("create");
        assert_eq!(user.token_version, 0);

        let now = OffsetDateTime::now_utc();
        store
            .insert_session(&DbSession {
                cookie_id: "c1".to_string(),
                user_id: user.id.clone(),
                token_version: 0,
                issued_at: now,
                expires_at: now + time::Duration::seconds(3600),
                created_ip: None,
                revoked_at: None,
            })
            .expect("session");

        store
            .set_user_password(&user.id, "new-hash-line")
            .expect("password");
        let bumped = store.get_user_by_id("u1").expect("get").expect("present");
        assert_eq!(bumped.token_version, 1);

        // Explicit revocation marks the row.
        store.revoke_session("c1").expect("revoke");
        let session = store.get_session("c1").expect("get").expect("present");
        assert!(session.revoked_at.is_some());
    }

    #[test]
    fn deleting_user_cascades_sessions() {
        let store = MetadataStore::open_in_memory().expect("open");
        let user = store
            .create_user("u9", "carol", "hash", "admin", "")
            .expect("create");
        let now = OffsetDateTime::now_utc();
        store
            .insert_session(&DbSession {
                cookie_id: "c2".to_string(),
                user_id: user.id.clone(),
                token_version: 0,
                issued_at: now,
                expires_at: now + time::Duration::seconds(3600),
                created_ip: None,
                revoked_at: None,
            })
            .expect("session");
        store.delete_user(&user.id).expect("delete");
        assert!(store.get_session("c2").expect("session").is_none());
        assert_eq!(store.user_count().expect("count"), 0);
    }

    #[test]
    fn auth_tables_created_and_migrate_is_idempotent() {
        let store = MetadataStore::open_in_memory().expect("open");
        assert_eq!(store.schema_version().expect("version"), SCHEMA_VERSION);
        // Re-running migrate is a no-op and leaves everything intact.
        store.migrate().expect("migrate");
        assert_eq!(store.schema_version().expect("version"), SCHEMA_VERSION);
        store
            .create_user("uA", "dave", "hash", "admin", "")
            .expect("create after migrate");
    }
}
