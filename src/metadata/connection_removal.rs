use super::rows::timestamp_now;
use super::{MetadataError, MetadataStore};
use crate::durable::now;
use crate::manifest::{CommitState, TelegramLocation};
use rusqlite::{OptionalExtension, params};
use serde::Serialize;
use uuid::Uuid;

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub struct ConnectionRemovalJob {
    pub id: String,
    pub connection_id: String,
    pub delete_uploaded_files: bool,
    pub state: String,
    pub object_count: u64,
    pub requested_at: i64,
    pub updated_at: i64,
    pub completed_at: Option<i64>,
    pub error: Option<String>,
}

impl MetadataStore {
    /// Hide the current storage namespace immediately and record enough durable
    /// information for the worker to finish local and optional Telegram cleanup.
    pub fn begin_connection_removal(
        &self,
        delete_uploaded_files: bool,
    ) -> Result<ConnectionRemovalJob, MetadataError> {
        self.with_connection(|connection| {
            let tx = connection.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
            let connection_id = tx
                .query_row(
                    "SELECT value FROM app_settings WHERE key='telegram_active_connection_id'",
                    [],
                    |row| row.get::<_, String>(0),
                )
                .optional()?
                .unwrap_or_else(|| "legacy".to_string());
            let active: Option<String> = tx
                .query_row(
                    "SELECT id FROM connection_removal_jobs WHERE state IN ('pending','running','remote_cleanup_pending') ORDER BY requested_at LIMIT 1",
                    [],
                    |row| row.get(0),
                )
                .optional()?;
            if active.is_some() {
                return Err(MetadataError::ConnectionRemovalInProgress);
            }

            let id = Uuid::new_v4().to_string();
            let requested_at = now();
            tx.execute(
                "INSERT INTO connection_removal_jobs(id,connection_id,delete_uploaded_files,state,object_count,requested_at,updated_at) VALUES (?1,?2,?3,'pending',0,?4,?4)",
                params![id, connection_id, delete_uploaded_files, requested_at],
            )?;

            let mut manifests = Vec::new();
            {
                let mut statement = tx.prepare(
                    "SELECT object_id,manifest_json,commit_state FROM object_manifests WHERE connection_id=?1 AND commit_state != 'tombstoned' ORDER BY object_id",
                )?;
                let rows = statement.query_map([&connection_id], |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, String>(2)?,
                    ))
                })?;
                for row in rows {
                    let (object_id, json, state) = row?;
                    let manifest = serde_json::from_str::<crate::manifest::ObjectManifest>(&json)?;
                    manifest
                        .validate()
                        .map_err(MetadataError::InvalidManifest)?;
                    manifests.push((object_id, manifest, state));
                }
            }

            for (object_id, manifest, _state) in &mut manifests {
                manifest.commit_state = CommitState::Tombstoned;
                if delete_uploaded_files {
                    crate::durable::enqueue_manifest_cleanup_at(&tx, &*manifest, requested_at)?;
                }
                tx.execute(
                    "INSERT INTO connection_removal_objects(job_id,object_id) VALUES (?1,?2)",
                    params![id, &*object_id],
                )?;
                let tombstoned_at = timestamp_now()?;
                let manifest_json = serde_json::to_string(&*manifest)?;
                tx.execute(
                    "UPDATE object_manifests SET commit_state='tombstoned',manifest_json=?2,tombstoned_at=?3 WHERE object_id=?1",
                    params![&*object_id, manifest_json, tombstoned_at],
                )?;
                tx.execute(
                    "DELETE FROM recovery_markers WHERE object_id=?1",
                    [object_id.as_str()],
                )?;
            }

            let mut transfer_jobs = Vec::new();
            {
                let mut statement = tx.prepare(
                    "SELECT id,object_id FROM transfer_jobs WHERE connection_id=?1 ORDER BY sequence",
                )?;
                let rows = statement.query_map([&connection_id], |row| {
                    Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
                })?;
                for row in rows {
                    transfer_jobs.push(row?);
                }
            }
            for (job_id, object_id) in &transfer_jobs {
                tx.execute(
                    "INSERT OR IGNORE INTO connection_removal_objects(job_id,object_id) VALUES (?1,?2)",
                    params![id, object_id],
                )?;
                if delete_uploaded_files {
                    tx.execute(
                        "INSERT OR IGNORE INTO cleanup_targets(object_id,connection_id,peer_id,message_id,target_kind,due_at) VALUES (?1,?2,?3,0,'evidence',?4)",
                        params![object_id, connection_id, format!("evidence:{object_id}"), requested_at],
                    )?;
                    let mut locations = Vec::new();
                    for table in ["transfer_chunks", "transfer_send_attempts"] {
                        let sql = format!(
                            "SELECT location_json FROM {table} WHERE job_id=?1 AND location_json IS NOT NULL"
                        );
                        let mut statement = tx.prepare(&sql)?;
                        let rows = statement.query_map([job_id], |row| row.get::<_, String>(0))?;
                        for row in rows {
                            if let Ok(location) = serde_json::from_str::<TelegramLocation>(&row?) {
                                locations.push(location);
                            }
                        }
                    }
                    for location in locations {
                        if location.message_id > 0 {
                            tx.execute(
                                "INSERT OR IGNORE INTO cleanup_targets(object_id,connection_id,peer_id,message_id,target_kind,due_at) VALUES (?1,?2,?3,?4,'message',?5)",
                                params![object_id, connection_id, location.peer_id, location.message_id, requested_at],
                            )?;
                        }
                    }
                }
            }

            let mut multipart_uploads = Vec::new();
            {
                let mut statement = tx.prepare(
                    "SELECT upload_id FROM multipart_uploads WHERE connection_id=?1 AND state NOT IN ('completed','aborted') ORDER BY upload_id",
                )?;
                let rows = statement.query_map([&connection_id], |row| row.get::<_, String>(0))?;
                for row in rows {
                    multipart_uploads.push(row?);
                }
            }
            for upload_id in &multipart_uploads {
                tx.execute(
                    "INSERT INTO connection_removal_objects(job_id,object_id) VALUES (?1,?2)",
                    params![id, upload_id],
                )?;
                if delete_uploaded_files {
                    tx.execute(
                        "INSERT OR IGNORE INTO cleanup_targets(object_id,connection_id,peer_id,message_id,target_kind,due_at) VALUES (?1,?2,?3,0,'evidence',?4)",
                        params![upload_id, connection_id, format!("evidence:{upload_id}"), requested_at],
                    )?;
                    let mut statement = tx.prepare(
                        "SELECT part_json FROM multipart_parts WHERE upload_id=?1 ORDER BY part_number",
                    )?;
                    let rows = statement.query_map([upload_id], |row| row.get::<_, String>(0))?;
                    for row in rows {
                        let part = serde_json::from_str::<crate::multipart::MultipartPart>(&row?)?;
                        if part.telegram.message_id > 0 {
                            tx.execute(
                                "INSERT OR IGNORE INTO cleanup_targets(object_id,connection_id,peer_id,message_id,target_kind,due_at) VALUES (?1,?2,?3,?4,'message',?5)",
                                params![upload_id, connection_id, part.telegram.peer_id, part.telegram.message_id, requested_at],
                            )?;
                        }
                    }
                }
            }

            tx.execute(
                "DELETE FROM active_objects WHERE object_id IN (SELECT object_id FROM object_manifests WHERE connection_id=?1)",
                [&connection_id],
            )?;
            tx.execute(
                "UPDATE buckets SET deleted_at=COALESCE(deleted_at,?2) WHERE connection_id=?1 AND deleted_at IS NULL",
                params![connection_id, timestamp_now()?],
            )?;
            tx.execute(
                "UPDATE transfer_jobs SET state='cancelled',lease=NULL,lease_until=0,error='connection removed',updated_at=?2 WHERE connection_id=?1 AND state NOT IN ('completed','cancelled','cleaned')",
                params![connection_id, requested_at],
            )?;
            tx.execute(
                "UPDATE multipart_uploads SET state='aborted',updated_at=?2 WHERE connection_id=?1 AND state NOT IN ('completed','aborted')",
                params![connection_id, timestamp_now()?],
            )?;
            tx.execute(
                "UPDATE operation_journal SET state='cancelled',error='connection removed',updated_at=?2 WHERE object_id IN (SELECT object_id FROM connection_removal_objects WHERE job_id=?1) AND state='staging'",
                params![id, requested_at],
            )?;
            tx.execute(
                "DELETE FROM recovery_markers WHERE object_id IN (SELECT object_id FROM connection_removal_objects WHERE job_id=?1)",
                [&id],
            )?;
            let object_count: u64 = tx.query_row(
                "SELECT COUNT(*) FROM connection_removal_objects WHERE job_id=?1",
                [&id],
                |row| row.get(0),
            )?;
            tx.execute(
                "UPDATE connection_removal_jobs SET object_count=?2,updated_at=?3 WHERE id=?1",
                params![id, object_count, requested_at],
            )?;
            tx.commit()?;
            Ok(ConnectionRemovalJob {
                id,
                connection_id,
                delete_uploaded_files,
                state: "pending".to_string(),
                object_count,
                requested_at,
                updated_at: requested_at,
                completed_at: None,
                error: None,
            })
        })
    }

    pub fn connection_removal_job(&self) -> Result<Option<ConnectionRemovalJob>, MetadataError> {
        self.with_connection(|connection| {
            connection
                .query_row(
                    "SELECT id,connection_id,delete_uploaded_files,state,object_count,requested_at,updated_at,completed_at,error FROM connection_removal_jobs WHERE state NOT IN ('completed') ORDER BY CASE WHEN state IN ('pending','running') THEN 0 ELSE 1 END, requested_at LIMIT 1",
                    [],
                    connection_removal_row,
                )
                .optional()
                .map_err(MetadataError::from)
        })
    }

    pub(crate) fn connection_removal_in_progress(&self) -> Result<bool, MetadataError> {
        self.with_connection(|connection| {
            Ok(connection.query_row(
                "SELECT EXISTS(SELECT 1 FROM connection_removal_jobs WHERE state IN ('pending','running','remote_cleanup_pending'))",
                [],
                |row| row.get(0),
            )?)
        })
    }

    pub(crate) fn connection_removal_cleanup_pending(
        &self,
        job_id: &str,
    ) -> Result<bool, MetadataError> {
        self.with_connection(|connection| {
            Ok(connection.query_row(
                "SELECT EXISTS(SELECT 1 FROM connection_removal_objects o JOIN cleanup_targets c ON c.object_id=o.object_id WHERE o.job_id=?1 AND c.completed=0)",
                [job_id],
                |row| row.get(0),
            )?)
        })
    }

    pub(crate) fn connection_removal_objects(
        &self,
        job_id: &str,
    ) -> Result<Vec<String>, MetadataError> {
        self.with_connection(|connection| {
            let mut statement = connection.prepare(
                "SELECT object_id FROM connection_removal_objects WHERE job_id=?1 ORDER BY object_id",
            )?;
            let rows = statement.query_map([job_id], |row| row.get(0))?;
            rows.collect::<Result<Vec<String>, _>>().map_err(MetadataError::from)
        })
    }

    pub(crate) fn finish_connection_removal(
        &self,
        job_id: &str,
        state: &str,
        error: Option<&str>,
    ) -> Result<(), MetadataError> {
        self.with_connection(|connection| {
            connection.execute(
                "UPDATE connection_removal_jobs SET state=?2,updated_at=?3,completed_at=CASE WHEN ?2='completed' THEN ?3 ELSE completed_at END,error=?4 WHERE id=?1 AND state NOT IN ('completed')",
                params![job_id, state, now(), error],
            )?;
            Ok(())
        })
    }
}

fn connection_removal_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<ConnectionRemovalJob> {
    Ok(ConnectionRemovalJob {
        id: row.get(0)?,
        connection_id: row.get(1)?,
        delete_uploaded_files: row.get(2)?,
        state: row.get(3)?,
        object_count: row.get(4)?,
        requested_at: row.get(5)?,
        updated_at: row.get(6)?,
        completed_at: row.get(7)?,
        error: row.get(8)?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::manifest::CommittedManifestArgs;
    use crate::metadata::{BucketRecord, OperationKind};
    use time::OffsetDateTime;

    fn store_with_object() -> MetadataStore {
        let store = MetadataStore::open_in_memory().expect("open");
        store
            .create_bucket(BucketRecord {
                name: "bucket".to_string(),
                created_at: OffsetDateTime::now_utc(),
                deleted_at: None,
                versioning_enabled: false,
                object_locking_enabled: false,
            })
            .expect("bucket");
        let manifest = crate::manifest::ObjectManifest::committed(CommittedManifestArgs {
            bucket: "bucket".to_string(),
            key: "file.txt".to_string(),
            content_length: 4,
            content_type: "text/plain".to_string(),
            checksum_algorithm: "sha256".to_string(),
            whole_object: "abcd".to_string(),
            peer_id: "peer".to_string(),
            message_id: 10,
        });
        let operation = store
            .stage_manifest(OperationKind::Put, manifest)
            .expect("stage");
        store.commit_manifest(operation).expect("commit");
        store
    }

    #[test]
    fn removal_hides_namespace_and_queues_remote_cleanup() {
        let store = store_with_object();
        store
            .set_telegram_bootstrap_settings(&crate::metadata::TelegramBootstrapSettings {
                telegram_api_id: Some("123".into()),
                telegram_api_hash: Some("hash".into()),
                telegram_storage_chat_id: Some("-1001234567890".into()),
                ..Default::default()
            })
            .expect("settings");
        let job = store.begin_connection_removal(true).expect("begin removal");
        assert!(job.delete_uploaded_files);
        assert_ne!(job.connection_id, "legacy");
        assert_eq!(job.object_count, 1);
        assert!(store.list_buckets().expect("buckets").is_empty());
        assert_eq!(store.status().expect("status").active_objects, 0);
        assert!(
            store
                .connection_removal_cleanup_pending(&job.id)
                .expect("pending")
        );
        store
            .finish_connection_removal(&job.id, "remote_cleanup_pending", None)
            .expect("remote pending");
        assert!(matches!(
            store.begin_connection_removal(false),
            Err(MetadataError::ConnectionRemovalInProgress)
        ));
    }

    #[test]
    fn removal_without_remote_delete_still_hides_local_data() {
        let store = store_with_object();
        let job = store
            .begin_connection_removal(false)
            .expect("begin removal");
        assert!(!job.delete_uploaded_files);
        assert!(
            !store
                .connection_removal_cleanup_pending(&job.id)
                .expect("pending")
        );
        assert!(
            store
                .list_manifests()
                .expect("manifests")
                .iter()
                .all(|manifest| {
                    manifest.commit_state == crate::manifest::CommitState::Tombstoned
                })
        );
    }

    #[test]
    fn removal_scopes_and_purges_attention_transfer_state() {
        let store = MetadataStore::open_in_memory().expect("open");
        store
            .set_telegram_bootstrap_settings(&crate::metadata::TelegramBootstrapSettings {
                telegram_api_id: Some("123".into()),
                telegram_api_hash: Some("hash".into()),
                telegram_storage_chat_id: Some("-1001234567890".into()),
                ..Default::default()
            })
            .expect("settings");
        store
            .create_bucket(BucketRecord {
                name: "bucket".to_string(),
                created_at: OffsetDateTime::now_utc(),
                deleted_at: None,
                versioning_enabled: false,
                object_locking_enabled: false,
            })
            .expect("bucket");

        let removed_job_id = Uuid::new_v4();
        store
            .begin_transfer(removed_job_id, "bucket", "removed.bin")
            .expect("removed transfer");
        store
            .with_connection(|connection| {
                connection.execute(
                    "INSERT INTO operation_journal(operation_id,object_id,bucket,object_key,operation_kind,state,created_at,updated_at) VALUES (?1,?2,'bucket','removed.bin','put','staging','1','1')",
                    rusqlite::params![Uuid::new_v4().to_string(), removed_job_id.to_string()],
                )?;
                connection.execute(
                    "INSERT INTO recovery_markers(marker_key,object_id,bucket,object_key,marker_state,details_json,created_at,updated_at) VALUES (?1,?2,'bucket','removed.bin','staging','{}','1','1')",
                    rusqlite::params![format!("staging:{}", Uuid::new_v4()), removed_job_id.to_string()],
                )?;
                Ok(())
            })
            .expect("staging state");
        let other_job_id = Uuid::new_v4();
        store
            .with_connection(|connection| {
                connection.execute(
                    "INSERT INTO transfer_jobs(id,object_id,connection_id,bucket,object_key,created_at,updated_at,lease_until) VALUES (?1,?1,'other-connection','other','other.bin',1,1,0)",
                    [other_job_id.to_string()],
                )?;
                Ok(())
            })
            .expect("other transfer");

        let removal = store
            .begin_connection_removal(false)
            .expect("begin removal");
        assert_eq!(removal.object_count, 1);
        assert!(
            store
                .transfers(50, 0)
                .expect("visible transfers")
                .is_empty()
        );

        let removed_state: String = store
            .with_connection(|connection| {
                Ok(connection.query_row(
                    "SELECT state FROM transfer_jobs WHERE id=?1",
                    [removed_job_id.to_string()],
                    |row| row.get(0),
                )?)
            })
            .expect("removed state");
        assert_eq!(removed_state, "cancelled");
        let restart_state: (String, u64) = store
            .with_connection(|connection| {
                let journal_state = connection.query_row(
                    "SELECT state FROM operation_journal WHERE object_id=?1",
                    [removed_job_id.to_string()],
                    |row| row.get(0),
                )?;
                let marker_count = connection.query_row(
                    "SELECT COUNT(*) FROM recovery_markers WHERE object_id=?1",
                    [removed_job_id.to_string()],
                    |row| row.get(0),
                )?;
                Ok((journal_state, marker_count))
            })
            .expect("restart state");
        assert_eq!(restart_state, ("cancelled".to_string(), 0));
        store.startup_reconcile().expect("restart reconcile");
        assert_eq!(store.rebuild_index().expect("rebuild").recovery_markers, 0);

        store
            .purge_connection_operations(&removal.connection_id)
            .expect("purge removed operations");
        let remaining_other = store
            .with_connection(|connection| {
                Ok(connection.query_row(
                    "SELECT COUNT(*) FROM transfer_jobs WHERE id=?1 AND connection_id='other-connection'",
                    [other_job_id.to_string()],
                    |row| row.get::<_, u64>(0),
                )?)
            })
            .expect("other state");
        assert_eq!(remaining_other, 1);
        assert!(
            store
                .transfer(&removed_job_id.to_string())
                .expect("removed lookup")
                .is_none()
        );
    }

    #[test]
    fn removal_preserves_known_transfer_locations_for_remote_cleanup() {
        let store = MetadataStore::open_in_memory().expect("open");
        store
            .set_telegram_bootstrap_settings(&crate::metadata::TelegramBootstrapSettings {
                telegram_api_id: Some("123".into()),
                telegram_api_hash: Some("hash".into()),
                telegram_storage_chat_id: Some("-1001234567890".into()),
                ..Default::default()
            })
            .expect("settings");
        store
            .create_bucket(BucketRecord {
                name: "bucket".to_string(),
                created_at: OffsetDateTime::now_utc(),
                deleted_at: None,
                versioning_enabled: false,
                object_locking_enabled: false,
            })
            .expect("bucket");
        let transfer_id = Uuid::new_v4();
        store
            .begin_transfer(transfer_id, "bucket", "known.bin")
            .expect("transfer");
        let location = serde_json::to_string(&TelegramLocation {
            peer_id: "peer".into(),
            message_id: 44,
            document_id: Some("document".into()),
        })
        .expect("location json");
        store
            .with_connection(|connection| {
                connection.execute(
                    "INSERT INTO transfer_chunks(job_id,chunk_order,location_json) VALUES (?1,0,?2)",
                    rusqlite::params![transfer_id.to_string(), location],
                )?;
                Ok(())
            })
            .expect("location");

        let removal = store.begin_connection_removal(true).expect("begin removal");
        assert!(
            store
                .connection_removal_cleanup_pending(&removal.id)
                .expect("cleanup pending")
        );
        let target_count: u64 = store
            .with_connection(|connection| {
                Ok(connection.query_row(
                    "SELECT COUNT(*) FROM cleanup_targets WHERE object_id=?1 AND peer_id='peer' AND message_id=44",
                    [transfer_id.to_string()],
                    |row| row.get(0),
                )?)
            })
            .expect("target count");
        assert_eq!(target_count, 1);
    }
}
