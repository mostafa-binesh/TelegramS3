use super::rows::{
    JournalDetails, load_journal_entry, load_manifest_by_object_id, manifest_json_with_state,
    parse_rfc3339_timestamp, timestamp_now,
};
use super::{MetadataError, MetadataStore, OperationKind};
use crate::durable::TransferWriteConditionals;
use crate::manifest::{CommitState, ObjectManifest};
use rusqlite::{OptionalExtension, params};
use s3s::dto::{ETagCondition, Timestamp};
use std::str::FromStr;
use time::OffsetDateTime;
use uuid::Uuid;

type TransferJobFenceRow = (
    String,
    Option<String>,
    i64,
    i64,
    String,
    String,
    Option<String>,
);

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

impl MetadataStore {
    pub fn stage_manifest(
        &self,
        operation_kind: OperationKind,
        manifest: ObjectManifest,
    ) -> Result<Uuid, MetadataError> {
        manifest
            .validate()
            .map_err(MetadataError::InvalidManifest)?;

        let operation_id = manifest.object_id;
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
                    connection_id,
                    bucket,
                    object_key,
                    version_id,
                    commit_state,
                    manifest_json,
                    created_at,
                    committed_at,
                    tombstoned_at
                )
                VALUES (?1, COALESCE((SELECT value FROM app_settings WHERE key='telegram_active_connection_id'), 'legacy'), ?2, ?3, ?4, 'staging', ?5, ?6, NULL, NULL)
                ON CONFLICT(object_id) DO UPDATE SET
                    connection_id = excluded.connection_id,
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

        super::recovery::ensure_recovery_marker(self, &journal_details)?;
        Ok(operation_id)
    }

    pub fn commit_manifest(&self, operation_id: Uuid) -> Result<ObjectManifest, MetadataError> {
        self.commit_manifest_inner(operation_id, None, None)
    }

    pub(crate) fn commit_transfer_manifest(
        &self,
        operation_id: Uuid,
        job_id: &str,
        lease: &str,
        published: ObjectManifest,
    ) -> Result<ObjectManifest, MetadataError> {
        self.commit_manifest_inner(operation_id, Some((job_id, lease)), Some(published))
    }

    fn commit_manifest_inner(
        &self,
        operation_id: Uuid,
        transfer: Option<(&str, &str)>,
        published: Option<ObjectManifest>,
    ) -> Result<ObjectManifest, MetadataError> {
        let operation_id = operation_id.to_string();
        self.with_connection(|connection| {
            let tx = connection.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
            let journal = load_journal_entry(&tx, &operation_id)?
                .ok_or_else(|| MetadataError::JournalNotFound(operation_id.clone()))?;
            let mut manifest = load_manifest_by_object_id(&tx, &journal.object_id)?
                .ok_or_else(|| MetadataError::ManifestNotFound(journal.object_id.clone()))?;

            if journal.state == "committed" && manifest.commit_state == CommitState::Committed {
                return Ok(manifest);
            }

            if let Some(published) = published {
                published.validate().map_err(MetadataError::InvalidManifest)?;
                if published.object_id.to_string() != journal.object_id
                    || published.bucket != journal.bucket
                    || published.key != journal.object_key
                {
                    return Err(MetadataError::InvalidManifest(
                        "published manifest identity changed".into(),
                    ));
                }
                manifest = published;
            }

            let mut strict_job_sequence = None;
            if let Some((job_id, lease)) = transfer {
                let row: Option<TransferJobFenceRow> = tx
                    .query_row(
                        "SELECT state,lease,lease_until,sequence,bucket,object_key,operation_id FROM transfer_jobs WHERE id=?1",
                        [job_id],
                        |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?, r.get(5)?, r.get(6)?)),
                    )
                    .optional()?;
                let Some((state, actual_lease, lease_until, sequence, bucket, key, job_operation)) = row else {
                    return Err(MetadataError::InvalidManifest("transfer job missing".into()));
                };
                if state != "committing"
                    || actual_lease.as_deref() != Some(lease)
                    || lease_until < crate::durable::now()
                    || job_operation.as_deref() != Some(operation_id.as_str())
                {
                    return Err(MetadataError::InvalidManifest("publication fence lost".into()));
                }
                let older_pending: bool = tx.query_row(
                    "SELECT EXISTS(SELECT 1 FROM transfer_jobs WHERE bucket=?1 AND object_key=?2 AND sequence<?3 AND state NOT IN ('completed','cleaned','cancelled','reception_failed','superseded'))",
                    params![bucket, key, sequence],
                    |r| r.get(0),
                )?;
                let newer_committed: bool = tx.query_row(
                    "SELECT EXISTS(SELECT 1 FROM transfer_jobs WHERE bucket=?1 AND object_key=?2 AND sequence>?3 AND state IN ('completed','cleaned'))",
                    params![bucket, key, sequence],
                    |r| r.get(0),
                )?;
                if older_pending || newer_committed {
                    return Err(MetadataError::InvalidManifest(
                        "per-key publication order changed".into(),
                    ));
                }
                strict_job_sequence = Some((job_id, lease));
            } else {
                let job_state: Option<String> = tx.query_row("SELECT state FROM transfer_jobs WHERE operation_id=?1", [&operation_id], |r| r.get(0)).optional()?;
                if job_state.as_deref().is_some_and(|s| !matches!(s, "uploading" | "committing")) {
                    return Err(MetadataError::InvalidManifest("transfer cannot commit in current state".into()));
                }
            }
            let current_manifest = {
                let object_id: Option<String> = tx
                    .query_row(
                        "SELECT object_id FROM active_objects WHERE bucket=?1 AND object_key=?2",
                        params![journal.bucket, journal.object_key],
                        |row| row.get(0),
                    )
                    .optional()?;
                match object_id {
                    Some(object_id) => load_manifest_by_object_id(&tx, &object_id)?,
                    None => None,
                }
            };
            if let Some((job_id, _)) = transfer {
                let conditionals: Option<String> = tx.query_row(
                    "SELECT write_conditionals_json FROM transfer_jobs WHERE id=?1",
                    [job_id],
                    |row| row.get::<_, Option<String>>(0),
                )?;
                if let Some(conditionals) = conditionals {
                    let conditionals: TransferWriteConditionals =
                        serde_json::from_str(&conditionals)?;
                    enforce_write_conditionals_snapshot(
                        &conditionals,
                        current_manifest.as_ref(),
                    )?;
                }
            }
            let completion_job = transfer.map(|(job_id, _)| job_id).unwrap_or(&journal.object_id);
            let completion: Option<String> = tx.query_row(
                "SELECT upload_id FROM multipart_jobs WHERE job_id=?1 AND part_number=0",
                [completion_job],
                |r| r.get(0),
            ).optional()?;
            if let Some(upload) = completion.as_deref() {
                let active: bool = tx.query_row(
                    "SELECT EXISTS(SELECT 1 FROM multipart_uploads WHERE upload_id=?1 AND state IN ('initiated','uploading'))",
                    [upload],
                    |r| r.get(0),
                )?;
                if !active {
                    return Err(MetadataError::InvalidManifest(
                        "multipart completion session is closed".into(),
                    ));
                }
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
            if let Some((job_id, lease)) = strict_job_sequence {
                if tx.execute(
                    "UPDATE transfer_jobs SET state='completed',error=NULL,lease=NULL,lease_until=0,updated_at=?3 WHERE id=?1 AND lease=?2 AND state='committing'",
                    params![job_id, lease, crate::durable::now()],
                )? != 1 {
                    return Err(MetadataError::InvalidManifest("publication fence lost".into()));
                }
            } else {
                tx.execute("UPDATE transfer_jobs SET state='completed',error=NULL,lease=NULL,lease_until=0,updated_at=?2 WHERE operation_id=?1", params![operation_id, crate::durable::now()])?;
            }
            if let Some(upload) = completion {
                for row in tx.prepare("SELECT part_json FROM multipart_parts WHERE upload_id=?1")?.query_map([&upload],|r|r.get::<_,String>(0))? {
                    crate::durable::enqueue_part_cleanup(&tx,&serde_json::from_str(&row?)?)?;
                }
                if tx.execute("UPDATE multipart_uploads SET state='completed',session_json=json_set(session_json,'$.state','completed') WHERE upload_id=?1 AND state IN ('initiated','uploading')",[&upload])? != 1 {
                    return Err(MetadataError::InvalidManifest("multipart completion session is closed".into()));
                }
                tx.execute("DELETE FROM multipart_parts WHERE upload_id=?1",[upload])?;
            }
            tx.commit()?;
            Ok(manifest)
        })
    }

    pub fn update_manifest(
        &self,
        manifest: ObjectManifest,
    ) -> Result<ObjectManifest, MetadataError> {
        let object_id = manifest.object_id.to_string();
        let manifest_json = serde_json::to_string(&manifest)?;
        self.with_connection(|connection| {
            let tx = connection.transaction()?;
            tx.execute(
                r#"
                UPDATE object_manifests
                SET commit_state = ?2,
                    manifest_json = ?3
                WHERE object_id = ?1
                "#,
                params![object_id, manifest.commit_state.as_str(), manifest_json],
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
        self.tombstone_manifest_with_conditionals(object_id, reason, None, None, None)
    }

    pub fn tombstone_manifest_with_conditionals(
        &self,
        object_id: Uuid,
        reason: &str,
        if_match: Option<&ETagCondition>,
        if_match_last_modified_time: Option<&Timestamp>,
        if_match_size: Option<i64>,
    ) -> Result<ObjectManifest, MetadataError> {
        let object_id = object_id.to_string();
        self.with_connection(|connection| {
            let tx = connection.transaction()?;
            let mut manifest = load_manifest_by_object_id(&tx, &object_id)?
                .ok_or_else(|| MetadataError::ManifestNotFound(object_id.clone()))?;
            enforce_delete_conditionals_snapshot(
                if_match,
                if_match_last_modified_time,
                if_match_size,
                &manifest,
            )?;

            let tombstoned_at = timestamp_now()?;
            let bucket = manifest.bucket.clone();
            let object_key = manifest.key.clone();
            let manifest_object_id = manifest.object_id.to_string();
            manifest.commit_state = CommitState::Tombstoned;
            crate::durable::enqueue_manifest_cleanup(&tx, &manifest)?;
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

    /// Tombstone expired objects that are still the active version of their key.
    ///
    /// Expiry discovery and removal of the active pointer happen in one
    /// metadata transaction. Remote locations are handed to the existing
    /// evidence-first cleanup outbox.
    pub fn tombstone_expired_active_manifests(
        &self,
        now: OffsetDateTime,
    ) -> Result<u64, MetadataError> {
        self.with_connection(|connection| {
            let tx = connection.transaction()?;
            let candidates = {
                let mut statement = tx.prepare(
                    r#"
                    SELECT m.object_id, m.manifest_json
                    FROM active_objects a
                    JOIN object_manifests m ON m.object_id = a.object_id
                    WHERE m.commit_state = 'committed'
                      AND m.connection_id = COALESCE(
                          (SELECT value FROM app_settings WHERE key='telegram_active_connection_id'),
                          'legacy'
                      )
                    "#,
                )?;
                statement
                    .query_map([], |row| {
                        Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
                    })?
                    .collect::<Result<Vec<_>, _>>()?
            };
            let mut tombstoned = 0_u64;
            for (object_id, manifest_json) in candidates {
                let mut manifest: ObjectManifest = serde_json::from_str(&manifest_json)?;
                if !manifest.is_expired(now) {
                    continue;
                }
                let tombstoned_at = timestamp_now()?;
                manifest.commit_state = CommitState::Tombstoned;
                crate::durable::enqueue_manifest_cleanup(&tx, &manifest)?;
                let manifest_json = serde_json::to_string(&manifest)?;
                tx.execute(
                    r#"
                    UPDATE object_manifests
                    SET commit_state='tombstoned', manifest_json=?2, tombstoned_at=?3
                    WHERE object_id=?1 AND commit_state='committed'
                    "#,
                    params![object_id, manifest_json, tombstoned_at],
                )?;
                tx.execute(
                    "DELETE FROM active_objects WHERE object_id=?1",
                    params![object_id],
                )?;
                tx.execute(
                    "DELETE FROM recovery_markers WHERE object_id=?1",
                    params![object_id],
                )?;
                tx.execute(
                    r#"
                    UPDATE operation_journal
                    SET state='tombstoned', error='object expired', updated_at=?2
                    WHERE object_id=?1
                    "#,
                    params![object_id, tombstoned_at],
                )?;
                tombstoned += 1;
            }
            tx.commit()?;
            Ok(tombstoned)
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
                    SELECT a.object_id
                    FROM active_objects a
                    JOIN object_manifests m ON m.object_id = a.object_id
                    WHERE a.bucket = ?1 AND a.object_key = ?2
                      AND m.connection_id = COALESCE((SELECT value FROM app_settings WHERE key='telegram_active_connection_id'), 'legacy')
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

    pub fn list_manifests(&self) -> Result<Vec<ObjectManifest>, MetadataError> {
        self.with_connection(|connection| {
            let mut stmt = connection.prepare(
                r#"
                SELECT manifest_json
                FROM object_manifests
                WHERE connection_id = COALESCE((SELECT value FROM app_settings WHERE key='telegram_active_connection_id'), 'legacy')
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
}

fn etag_condition_matches(condition: &ETagCondition, actual: &str) -> bool {
    match condition {
        ETagCondition::Any => true,
        ETagCondition::ETag(etag) => {
            etag.as_strong() == Some(actual) || etag.as_weak() == Some(actual)
        }
    }
}

fn enforce_write_conditionals_snapshot(
    conditionals: &TransferWriteConditionals,
    current_manifest: Option<&ObjectManifest>,
) -> Result<(), MetadataError> {
    let Some(manifest) = current_manifest else {
        if conditionals.if_match.is_some() {
            return Err(MetadataError::InvalidManifest(
                "if-match precondition failed".into(),
            ));
        }
        return Ok(());
    };
    let current_etag = manifest.checksum.whole_object.as_str();
    if let Some(raw) = &conditionals.if_match {
        let condition = ETagCondition::from_str(raw).map_err(|_| {
            MetadataError::InvalidManifest("invalid stored if-match condition".into())
        })?;
        if !etag_condition_matches(&condition, current_etag) {
            return Err(MetadataError::InvalidManifest(
                "if-match precondition failed".into(),
            ));
        }
        return Ok(());
    }
    if let Some(raw) = &conditionals.if_none_match {
        let condition = ETagCondition::from_str(raw).map_err(|_| {
            MetadataError::InvalidManifest("invalid stored if-none-match condition".into())
        })?;
        if etag_condition_matches(&condition, current_etag) {
            return Err(MetadataError::InvalidManifest(
                "if-none-match precondition failed".into(),
            ));
        }
    }
    Ok(())
}

pub(super) fn enforce_delete_conditionals_snapshot(
    if_match: Option<&ETagCondition>,
    if_match_last_modified_time: Option<&Timestamp>,
    if_match_size: Option<i64>,
    manifest: &ObjectManifest,
) -> Result<(), MetadataError> {
    let current_etag = manifest.checksum.whole_object.as_str();
    if let Some(condition) = if_match
        && !etag_condition_matches(condition, current_etag)
    {
        return Err(MetadataError::PreconditionFailed(
            "if-match precondition failed".into(),
        ));
    }
    if let Some(condition) = if_match_last_modified_time {
        let expected: OffsetDateTime = condition.clone().into();
        if manifest.created_at != expected {
            return Err(MetadataError::PreconditionFailed(
                "if-match-last-modified-time precondition failed".into(),
            ));
        }
    }
    if let Some(condition) = if_match_size
        && manifest.content_length as i64 != condition
    {
        return Err(MetadataError::PreconditionFailed(
            "if-match-size precondition failed".into(),
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::durable::TransferWriteConditionals;
    use crate::manifest::CommittedManifestArgs;
    use rusqlite::params;

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
    fn expired_active_manifest_is_tombstoned_and_queued() {
        let store = MetadataStore::open_in_memory().expect("open");
        let mut manifest = sample_manifest("bucket", "expired.txt");
        manifest.expires_at = Some(time::OffsetDateTime::now_utc() - time::Duration::seconds(1));
        let operation_id = store
            .stage_manifest(OperationKind::Put, manifest.clone())
            .expect("stage");
        store.commit_manifest(operation_id).expect("commit");

        assert_eq!(
            store
                .tombstone_expired_active_manifests(time::OffsetDateTime::now_utc())
                .expect("expire"),
            1
        );
        assert!(
            store
                .get_active_manifest("bucket", "expired.txt")
                .expect("active")
                .is_none()
        );
        assert_eq!(
            store
                .get_manifest(manifest.object_id)
                .expect("history")
                .expect("manifest")
                .commit_state,
            CommitState::Tombstoned
        );
        assert!(store.claim_cleanup().expect("cleanup claim").is_some());
    }

    #[test]
    fn commit_transfer_rechecks_write_conditionals_before_visibility() {
        let store = MetadataStore::open_in_memory().expect("open");
        let bucket = store
            .create_bucket(crate::metadata::BucketRecord {
                name: "bucket".to_string(),
                created_at: time::OffsetDateTime::now_utc(),
                deleted_at: None,
                versioning_enabled: false,
                object_locking_enabled: false,
            })
            .expect("bucket");

        let current = sample_manifest(&bucket.name, "key.txt");
        let current_op = store
            .stage_manifest(OperationKind::Put, current.clone())
            .expect("stage current");
        store.commit_manifest(current_op).expect("commit current");

        let staged = sample_manifest(&bucket.name, "key.txt");
        let job_id = store
            .begin_transfer(staged.object_id, &bucket.name, &staged.key)
            .expect("begin transfer");
        store
            .set_transfer_conditionals(
                &job_id,
                Some(&TransferWriteConditionals {
                    if_match: Some(current.checksum.whole_object.clone()),
                    if_none_match: None,
                }),
            )
            .expect("record conditionals");
        let operation_id = store
            .stage_manifest(OperationKind::Put, staged.clone())
            .expect("stage staged");
        store
            .queue_transfer(&job_id, operation_id, staged.chunks.len())
            .expect("queue transfer");
        store.with_connection(|connection| {
            connection.execute(
                "UPDATE transfer_jobs SET state='committing', lease='lease', lease_until=?2 WHERE id=?1",
                params![job_id, crate::durable::now() + 120],
            )?;
            Ok(())
        }).expect("arm commit");

        let mut replacement = sample_manifest(&bucket.name, "key.txt");
        replacement.checksum.whole_object = "efgh".to_string();
        let replacement_op = store
            .stage_manifest(OperationKind::Put, replacement.clone())
            .expect("stage replacement");
        store
            .commit_manifest(replacement_op)
            .expect("commit replacement");

        let err = store
            .commit_transfer_manifest(operation_id, &job_id, "lease", staged)
            .expect_err("stale if-match should fail");
        assert!(matches!(err, MetadataError::InvalidManifest(_)));
    }
}
