use super::rows::{load_bucket_record, parse_rfc3339_timestamp, timestamp_now};
use super::{MetadataError, MetadataStore};
use crate::manifest::ObjectManifest;
use rusqlite::{OptionalExtension, params};
use time::OffsetDateTime;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BucketRecord {
    pub name: String,
    pub created_at: OffsetDateTime,
    pub deleted_at: Option<OffsetDateTime>,
    pub versioning_enabled: bool,
    pub object_locking_enabled: bool,
}

impl MetadataStore {
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
            let pending: bool = tx.query_row("SELECT EXISTS(SELECT 1 FROM transfer_jobs WHERE bucket=?1 AND state NOT IN ('completed','cancelled','cleaned'))",[bucket],|r|r.get(0))?;
            if pending {
                return Err(MetadataError::BucketNotEmpty(bucket.into()));
            }
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
}
