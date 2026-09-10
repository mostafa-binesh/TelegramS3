use super::rows::timestamp_now;
use super::{MetadataError, MetadataStore};
use crate::durable::now;
use crate::manifest::CommitState;
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
                    "SELECT object_id,manifest_json,commit_state FROM object_manifests WHERE commit_state != 'tombstoned' ORDER BY object_id",
                )?;
                let rows = statement.query_map([], |row| {
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
            }

            let mut multipart_uploads = Vec::new();
            {
                let mut statement = tx.prepare(
                    "SELECT upload_id FROM multipart_uploads WHERE state NOT IN ('completed','aborted') ORDER BY upload_id",
                )?;
                let rows = statement.query_map([], |row| row.get::<_, String>(0))?;
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

            tx.execute("DELETE FROM active_objects", [])?;
            tx.execute("DELETE FROM recovery_markers", [])?;
            tx.execute(
                "UPDATE buckets SET deleted_at=COALESCE(deleted_at,?1) WHERE deleted_at IS NULL",
                [timestamp_now()?],
            )?;
            tx.execute(
                "UPDATE transfer_jobs SET state='cancelled',error='connection removed',updated_at=?1 WHERE state NOT IN ('completed','cancelled','cleaned')",
                [requested_at],
            )?;
            tx.execute(
                "UPDATE multipart_uploads SET state='aborted',updated_at=?1 WHERE state NOT IN ('completed','aborted')",
                [timestamp_now()?],
            )?;
            tx.execute(
                "UPDATE connection_removal_jobs SET object_count=?2,updated_at=?3 WHERE id=?1",
                params![
                    id,
                    (manifests.len() + multipart_uploads.len()) as u64,
                    requested_at
                ],
            )?;
            tx.commit()?;
            Ok(ConnectionRemovalJob {
                id,
                connection_id,
                delete_uploaded_files,
                state: "pending".to_string(),
                object_count: (manifests.len() + multipart_uploads.len()) as u64,
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
}
