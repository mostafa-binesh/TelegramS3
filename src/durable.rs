//! Durable transfer state. SQLite transactions own admission, leases and checkpoints.
use crate::manifest::TelegramLocation;
use crate::metadata::{DbUser, MetadataError, MetadataStore};
use rusqlite::{OptionalExtension, params};
use serde::{Deserialize, Serialize};
use time::OffsetDateTime;
use uuid::Uuid;

pub(crate) fn now() -> i64 {
    OffsetDateTime::now_utc().unix_timestamp()
}

fn active_connection_id(tx: &rusqlite::Transaction<'_>) -> Result<String, MetadataError> {
    let has_settings: Option<String> = tx
        .query_row(
            "SELECT name FROM sqlite_master WHERE type='table' AND name='app_settings'",
            [],
            |row| row.get(0),
        )
        .optional()?;
    if has_settings.is_none() {
        return Ok("legacy".to_string());
    }
    Ok(tx
        .query_row(
            "SELECT value FROM app_settings WHERE key='telegram_active_connection_id'",
            [],
            |row| row.get(0),
        )
        .optional()?
        .unwrap_or_else(|| "legacy".to_string()))
}

#[derive(Debug, Clone, Serialize)]
pub struct TransferJob {
    pub id: String,
    pub object_id: String,
    pub operation_id: Option<String>,
    pub bucket: String,
    pub key: String,
    pub state: String,
    pub bytes: u64,
    pub chunks_done: u64,
    pub chunks_total: u64,
    pub attempts: u32,
    pub next_retry: i64,
    #[serde(skip)]
    pub lease: Option<String>,
    #[serde(skip)]
    pub write_conditionals_json: Option<String>,
    pub error: Option<String>,
    pub created_at: i64,
    pub updated_at: i64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SendAttempt {
    pub id: String,
    pub token: String,
    pub started_at: i64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct UnresolvedSendAttempt {
    pub job_id: String,
    pub order: u32,
    pub id: String,
    pub token: String,
    pub started_at: i64,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct TransferWriteConditionals {
    pub if_match: Option<String>,
    pub if_none_match: Option<String>,
}

#[derive(Debug, Clone)]
pub(crate) struct CleanupTarget {
    pub id: i64,
    pub object_id: String,
    pub connection_id: String,
    pub peer_id: String,
    pub message_id: i64,
    pub kind: String,
    pub lease: String,
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct DurableMetrics {
    pub pending_jobs: u64,
    pub oldest_pending_age_seconds: u64,
    pub retries: u64,
    pub failed_jobs: u64,
    pub staging_bytes: u64,
    pub cleanup_backlog: u64,
    pub cleanup_recovery_required: u64,
}

fn row_job(row: &rusqlite::Row<'_>) -> rusqlite::Result<TransferJob> {
    Ok(TransferJob {
        id: row.get(0)?,
        object_id: row.get(1)?,
        operation_id: row.get(2)?,
        bucket: row.get(3)?,
        key: row.get(4)?,
        state: row.get(5)?,
        bytes: row.get(6)?,
        chunks_done: row.get(7)?,
        chunks_total: row.get(8)?,
        attempts: row.get(9)?,
        next_retry: row.get(10)?,
        lease: row.get(11)?,
        write_conditionals_json: row.get(12)?,
        error: row.get(13)?,
        created_at: row.get(14)?,
        updated_at: row.get(15)?,
    })
}
const COLUMNS: &str = "id,object_id,operation_id,bucket,object_key,state,bytes,chunks_done,chunks_total,attempts,next_retry,lease,write_conditionals_json,error,created_at,updated_at";

pub(crate) fn enqueue_part_cleanup(
    tx: &rusqlite::Transaction<'_>,
    part: &crate::multipart::MultipartPart,
) -> Result<(), MetadataError> {
    let connection_id = active_connection_id(tx)?;
    let cleanup_object_id = part
        .manifest
        .as_ref()
        .map(|m| m.object_id.to_string())
        .unwrap_or_else(|| part.upload_id.to_string());
    tx.execute(
        "INSERT OR IGNORE INTO cleanup_targets(object_id,connection_id,peer_id,message_id,target_kind,due_at) VALUES (?1,?2,?3,0,'evidence',?4)",
        params![cleanup_object_id, connection_id, format!("evidence:{cleanup_object_id}"), now() + 7 * 86400],
    )?;
    let mut locations = vec![part.telegram.clone()];
    if let Some(m) = &part.manifest {
        locations.extend(m.chunks.iter().map(|c| TelegramLocation {
            peer_id: c.telegram_peer_id.clone(),
            message_id: c.telegram_message_id,
            document_id: c.telegram_document_id.clone(),
        }));
    }
    for l in locations {
        tx.execute("INSERT OR IGNORE INTO cleanup_targets(object_id,connection_id,peer_id,message_id,target_kind,due_at) VALUES (?1,?2,?3,?4,'message',?5)",params![cleanup_object_id,connection_id,l.peer_id,l.message_id,now()+7*86400])?;
    }
    Ok(())
}

pub(crate) fn serialize_write_conditionals(
    conditionals: &TransferWriteConditionals,
) -> Result<String, MetadataError> {
    Ok(serde_json::to_string(conditionals)?)
}

pub(crate) fn enqueue_manifest_cleanup(
    tx: &rusqlite::Transaction<'_>,
    manifest: &crate::manifest::ObjectManifest,
) -> Result<(), MetadataError> {
    // Deletes are visible immediately; the durable worker writes evidence
    // first, then removes Telegram messages with retryable outbox semantics.
    enqueue_manifest_cleanup_at(tx, manifest, now())
}

pub(crate) fn enqueue_manifest_cleanup_at(
    tx: &rusqlite::Transaction<'_>,
    manifest: &crate::manifest::ObjectManifest,
    due_at: i64,
) -> Result<(), MetadataError> {
    let object_id = manifest.object_id.to_string();
    let connection_id = active_connection_id(tx)?;
    tx.execute(
        "INSERT OR IGNORE INTO cleanup_targets(object_id,connection_id,peer_id,message_id,target_kind,due_at) VALUES (?1,?2,?3,0,'evidence',?4)",
        params![object_id, connection_id, format!("evidence:{object_id}"), due_at],
    )?;
    for chunk in &manifest.chunks {
        if chunk.telegram_message_id > 0 {
            tx.execute(
                "INSERT OR IGNORE INTO cleanup_targets(object_id,connection_id,peer_id,message_id,target_kind,due_at) VALUES (?1,?2,?3,?4,'message',?5)",
                params![object_id, connection_id, chunk.telegram_peer_id, chunk.telegram_message_id, due_at],
            )?;
        }
    }
    if manifest.telegram.message_id > 0 {
        tx.execute(
            "INSERT OR IGNORE INTO cleanup_targets(object_id,connection_id,peer_id,message_id,target_kind,due_at) VALUES (?1,?2,?3,?4,'message',?5)",
            params![object_id, connection_id, manifest.telegram.peer_id, manifest.telegram.message_id, due_at],
        )?;
    }
    Ok(())
}

impl MetadataStore {
    pub fn durable_metrics(&self) -> Result<DurableMetrics, MetadataError> {
        self.with_connection(|c| {
            let pending_jobs: u64 = c.query_row(
                "SELECT COUNT(*) FROM transfer_jobs WHERE state IN ('receiving','queued','uploading','committing','retry_wait')",
                [],
                |r| r.get(0),
            )?;
            let oldest: Option<i64> = c.query_row(
                "SELECT MIN(created_at) FROM transfer_jobs WHERE state IN ('receiving','queued','uploading','committing','retry_wait')",
                [],
                |r| r.get(0),
            )?;
            Ok(DurableMetrics {
                pending_jobs,
                oldest_pending_age_seconds: oldest
                    .map(|value| now().saturating_sub(value).max(0) as u64)
                    .unwrap_or(0),
                retries: c.query_row(
                    "SELECT COALESCE(SUM(CASE WHEN attempts>1 THEN attempts-1 ELSE 0 END),0) FROM transfer_jobs",
                    [],
                    |r| r.get(0),
                )?,
                failed_jobs: c.query_row(
                    "SELECT COUNT(*) FROM transfer_jobs WHERE state IN ('recovery_required','reception_failed')",
                    [],
                    |r| r.get(0),
                )?,
                staging_bytes: c.query_row(
                    "SELECT COALESCE(SUM(bytes),0) FROM transfer_jobs WHERE state!='cleaned'",
                    [],
                    |r| r.get(0),
                )?,
                cleanup_backlog: c.query_row(
                    "SELECT COUNT(*) FROM cleanup_targets WHERE completed=0",
                    [],
                    |r| r.get(0),
                )?,
                cleanup_recovery_required: c.query_row(
                    "SELECT COUNT(*) FROM cleanup_targets WHERE state='recovery_required'",
                    [],
                    |r| r.get(0),
                )?,
            })
        })
    }
    pub(crate) fn claim_cleanup(&self) -> Result<Option<CleanupTarget>, MetadataError> {
        self.with_connection(|c| {
            let tx = c.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
            let row: Option<(i64, String, String, String, i64, String)> = tx.query_row(
                "SELECT t.id,t.object_id,t.connection_id,t.peer_id,t.message_id,t.target_kind FROM cleanup_targets t WHERE t.completed=0 AND ((t.state IN ('pending','retry_wait') AND t.due_at<=?1 AND t.next_retry<=?1) OR (t.state='running' AND t.lease_until<?1)) AND (t.target_kind='evidence' OR EXISTS(SELECT 1 FROM cleanup_targets e WHERE e.object_id=t.object_id AND e.target_kind='evidence' AND e.completed=1)) ORDER BY CASE t.target_kind WHEN 'evidence' THEN 0 ELSE 1 END,t.id LIMIT 1",
                [now()],
                |r| Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?,r.get(4)?,r.get(5)?)),
            ).optional()?;
            let Some((id, object_id, connection_id, peer_id, message_id, kind)) = row else { return Ok(None); };
            let lease = Uuid::new_v4().to_string();
            tx.execute(
                "UPDATE cleanup_targets SET state='running',lease=?2,lease_until=?3+120,attempts=attempts+1,error=NULL WHERE id=?1",
                params![id, lease, now()],
            )?;
            tx.commit()?;
            Ok(Some(CleanupTarget { id, object_id, connection_id, peer_id, message_id, kind, lease }))
        })
    }

    pub(crate) fn complete_cleanup(
        &self,
        id: i64,
        lease: &str,
        location: Option<&TelegramLocation>,
    ) -> Result<bool, MetadataError> {
        self.with_connection(|c| {
            let location_json = location.map(serde_json::to_string).transpose()?;
            Ok(c.execute(
                "UPDATE cleanup_targets SET state='completed',completed=1,lease=NULL,lease_until=0,error=NULL,evidence_location_json=COALESCE(?3,evidence_location_json) WHERE id=?1 AND lease=?2 AND state='running'",
                params![id, lease, location_json],
            )? == 1)
        })
    }

    pub(crate) fn fail_cleanup(
        &self,
        id: i64,
        lease: &str,
        retry_after: u64,
        message: &str,
    ) -> Result<(), MetadataError> {
        self.with_connection(|c| {
            c.execute(
                "UPDATE cleanup_targets SET state='retry_wait',next_retry=?3,lease=NULL,lease_until=0,error=?4 WHERE id=?1 AND lease=?2 AND state='running'",
                params![id, lease, now().saturating_add(retry_after as i64), message],
            )?;
            Ok(())
        })
    }

    pub(crate) fn quarantine_cleanup(
        &self,
        id: i64,
        lease: &str,
        message: &str,
    ) -> Result<(), MetadataError> {
        self.with_connection(|c| {
            c.execute(
                "UPDATE cleanup_targets SET state='recovery_required',lease=NULL,lease_until=0,error=?3 WHERE id=?1 AND lease=?2 AND state='running'",
                params![id, lease, message],
            )?;
            Ok(())
        })
    }

    pub(crate) fn cleanup_complete_for_object(
        &self,
        object_id: &str,
    ) -> Result<bool, MetadataError> {
        self.with_connection(|c| {
            Ok(c.query_row(
                "SELECT NOT EXISTS(SELECT 1 FROM cleanup_targets WHERE object_id=?1 AND completed=0)",
                [object_id],
                |r| r.get(0),
            )?)
        })
    }

    pub(crate) fn make_cleanup_due(&self, object_id: &str) -> Result<(), MetadataError> {
        self.with_connection(|c| {
            c.execute(
                "UPDATE cleanup_targets SET due_at=?2,next_retry=0 WHERE object_id=?1 AND completed=0 AND state IN ('pending','retry_wait')",
                params![object_id, now()],
            )?;
            Ok(())
        })
    }

    pub(crate) fn renew_receiving(&self, id: &str) -> Result<bool, MetadataError> {
        self.with_connection(|c| {
            Ok(c.execute(
                "UPDATE transfer_jobs SET lease_until=?2+120,updated_at=?2 WHERE id=?1 AND state='receiving'",
                params![id, now()],
            )? == 1)
        })
    }

    pub(crate) fn fail_reception(
        &self,
        id: &str,
        files_removed: bool,
        message: &str,
    ) -> Result<bool, MetadataError> {
        self.with_connection(|c| {
            let tx = c.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
            let changed = tx.execute(
                "UPDATE transfer_jobs SET state=?2,bytes=CASE WHEN ?3 THEN 0 ELSE bytes END,lease=NULL,lease_until=0,error=?4,updated_at=?5 WHERE id=?1 AND state='receiving'",
                params![id, if files_removed { "reception_failed" } else { "recovery_required" }, files_removed, message, now()],
            )? == 1;
            if changed {
                tx.execute("DELETE FROM recovery_markers WHERE object_id=?1", [id])?;
                tx.execute("DELETE FROM operation_journal WHERE object_id=?1 AND state='staging'", [id])?;
                tx.execute("DELETE FROM object_manifests WHERE object_id=?1 AND commit_state='staging'", [id])?;
            }
            tx.commit()?;
            Ok(changed)
        })
    }

    pub(crate) fn multipart_job(&self, id: &str) -> Result<Option<(String, u32)>, MetadataError> {
        self.with_connection(|c| {
            Ok(c.query_row(
                "SELECT upload_id,part_number FROM multipart_jobs WHERE job_id=?1",
                [id],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()?)
        })
    }
    pub(crate) fn finish_part_job(
        &self,
        id: &str,
        lease: &str,
        upload: &str,
        number: u32,
    ) -> Result<(), MetadataError> {
        if number == 0 {
            // Completion's local visibility and session transition are handled by commit_manifest.
            let operation = self
                .transfer(id)?
                .and_then(|j| j.operation_id)
                .ok_or_else(|| {
                    MetadataError::InvalidManifest("completion operation missing".into())
                })?;
            self.commit_manifest(
                Uuid::parse_str(&operation)
                    .map_err(|e| MetadataError::InvalidManifest(e.to_string()))?,
            )?;
            return Ok(());
        }
        self.with_connection(|c| {
            let tx=c.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
            let json:String=tx.query_row("SELECT manifest_json FROM object_manifests WHERE object_id=?1",[id],|r|r.get(0))?;
            let manifest:crate::manifest::ObjectManifest=serde_json::from_str(&json)?;
            let active:bool=tx.query_row("SELECT EXISTS(SELECT 1 FROM multipart_uploads WHERE upload_id=?1 AND state IN ('initiated','uploading'))",[upload],|r|r.get(0))?;
            if !active{return Err(MetadataError::InvalidManifest("multipart session is closed".into()));}
            let old:Option<String>=tx.query_row("SELECT part_json FROM multipart_parts WHERE upload_id=?1 AND part_number=?2",params![upload,number],|r|r.get(0)).optional()?;
            if let Some(old)=old { enqueue_part_cleanup(&tx,&serde_json::from_str(&old)?)?; }
            let part=crate::multipart::MultipartPart {upload_id:Uuid::parse_str(upload).map_err(|e|MetadataError::InvalidManifest(e.to_string()))?,part_number:number,size:manifest.content_length,checksum:manifest.checksum.whole_object.clone(),e_tag:manifest.checksum.whole_object.clone(),telegram:manifest.telegram.clone(),manifest:Some(manifest),created_at:OffsetDateTime::now_utc()};
            tx.execute("INSERT INTO multipart_parts(upload_id,part_number,part_json,created_at) VALUES (?1,?2,?3,?4) ON CONFLICT(upload_id,part_number) DO UPDATE SET part_json=excluded.part_json,created_at=excluded.created_at",params![upload,number,serde_json::to_string(&part)?,part.created_at.format(&time::format_description::well_known::Rfc3339).unwrap_or_default()])?;
            if tx.execute("UPDATE transfer_jobs SET state='completed',lease=NULL,error=NULL WHERE id=?1 AND lease=?2 AND state='committing'",params![id,lease])? != 1 {return Err(MetadataError::InvalidManifest("part lease lost".into()));}
            tx.execute("DELETE FROM recovery_markers WHERE object_id=?1", [id])?;
            tx.execute("DELETE FROM operation_journal WHERE object_id=?1", [id])?;
            tx.execute("DELETE FROM object_manifests WHERE object_id=?1", [id])?;
            tx.commit()?; Ok(())
        })
    }
    pub(crate) fn abort_parts(&self, upload: Uuid) -> Result<(), MetadataError> {
        self.with_connection(|c| {
            let tx=c.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
            let jobs:bool=tx.query_row("SELECT EXISTS(SELECT 1 FROM transfer_jobs j JOIN multipart_jobs p ON p.job_id=j.id WHERE p.upload_id=?1 AND j.state IN ('uploading','committing'))",[upload.to_string()],|r|r.get(0))?;
            if jobs {return Err(MetadataError::InvalidManifest("multipart publication in progress; retry abort".into()));}
            for json in tx.prepare("SELECT part_json FROM multipart_parts WHERE upload_id=?1")?.query_map([upload.to_string()],|r|r.get::<_,String>(0))? {enqueue_part_cleanup(&tx,&serde_json::from_str(&json?)?)?;}
            tx.execute("UPDATE transfer_jobs SET state='cancelled',lease=NULL WHERE id IN (SELECT job_id FROM multipart_jobs WHERE upload_id=?1) AND state NOT IN ('completed','cleaned')",[upload.to_string()])?;
            tx.execute("DELETE FROM multipart_parts WHERE upload_id=?1",[upload.to_string()])?;
            tx.execute("DELETE FROM multipart_uploads WHERE upload_id=?1",[upload.to_string()])?;
            tx.commit()?;Ok(())
        })
    }
    pub fn setup_required(&self) -> Result<bool, MetadataError> {
        self.with_connection(|c| Ok(c.query_row("SELECT value = 'false' AND NOT EXISTS(SELECT 1 FROM users) FROM app_settings WHERE key='setup_complete'", [], |r| r.get(0))?))
    }

    pub fn create_first_user(
        &self,
        username: &str,
        password_hash: &str,
        display: &str,
    ) -> Result<DbUser, MetadataError> {
        let id = Uuid::new_v4().to_string();
        let timestamp = OffsetDateTime::now_utc()
            .format(&time::format_description::well_known::Rfc3339)
            .map_err(|e| MetadataError::InvalidManifest(e.to_string()))?;
        self.with_connection(|c| {
            let tx = c.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
            let changed = tx.execute("UPDATE app_settings SET value='true' WHERE key='setup_complete' AND value='false' AND NOT EXISTS(SELECT 1 FROM users)", [])?;
            if changed != 1 { return Err(MetadataError::InvalidManifest("setup is already complete".into())); }
            tx.execute("INSERT INTO users(id,username,password_hash,role,display_name,disabled,token_version,created_at,updated_at) VALUES (?1,?2,?3,'superadmin',?4,0,0,?5,?5)", params![id,username,password_hash,display,timestamp])?;
            tx.commit()?;
            Ok(())
        })?;
        self.get_user_by_id(&id)?
            .ok_or_else(|| MetadataError::InvalidManifest("created account missing".into()))
    }

    pub fn begin_transfer(
        &self,
        object_id: Uuid,
        bucket: &str,
        key: &str,
    ) -> Result<String, MetadataError> {
        let id = object_id.to_string();
        self.with_connection(|c| {
            c.execute("INSERT INTO transfer_jobs(id,object_id,bucket,object_key,created_at,updated_at,lease_until) SELECT ?1,?1,?2,?3,?4,?4,?4+120 WHERE EXISTS(SELECT 1 FROM buckets WHERE name=?2 AND deleted_at IS NULL)", params![id,bucket,key,now()])?
                .eq(&1).then_some(()).ok_or_else(|| MetadataError::BucketNotFound(bucket.into()))
        })?;
        Ok(id)
    }

    pub fn reserve_staging(&self, id: &str, bytes: u64, budget: u64) -> Result<(), MetadataError> {
        self.with_connection(|c| {
            let tx = c.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
            let used: u64 = tx.query_row("SELECT COALESCE(SUM(bytes),0) FROM transfer_jobs WHERE state NOT IN ('completed','cleaned')", [], |r| r.get(0))?;
            if used.saturating_add(bytes) > budget { return Err(MetadataError::InvalidManifest("staging capacity exhausted".into())); }
            if tx.execute("UPDATE transfer_jobs SET bytes=bytes+?2, updated_at=?3, lease_until=?3+120 WHERE id=?1 AND state='receiving'",params![id,bytes,now()])? != 1 {
                return Err(MetadataError::InvalidManifest("upload is no longer receiving".into()));
            }
            tx.commit()?;
            Ok(())
        })
    }

    pub fn queue_transfer(
        &self,
        id: &str,
        operation: Uuid,
        chunks: usize,
    ) -> Result<(), MetadataError> {
        self.with_connection(|c| {
            if c.execute("UPDATE transfer_jobs SET operation_id=?2,chunks_total=?3,state='queued',updated_at=?4,lease_until=0 WHERE id=?1 AND state='receiving'",params![id,operation.to_string(),chunks as i64,now()])? != 1 {
                return Err(MetadataError::InvalidManifest("upload cannot be queued".into()));
            }
            Ok(())
        })
    }

    pub fn set_transfer_conditionals(
        &self,
        id: &str,
        conditionals: Option<&TransferWriteConditionals>,
    ) -> Result<(), MetadataError> {
        let json = conditionals.map(serialize_write_conditionals).transpose()?;
        self.with_connection(|c| {
            if c.execute(
                "UPDATE transfer_jobs SET write_conditionals_json=?2,updated_at=?3 WHERE id=?1 AND state='receiving'",
                params![id, json, now()],
            )? != 1 {
                return Err(MetadataError::InvalidManifest(
                    "transfer conditionals could not be recorded".into(),
                ));
            }
            Ok(())
        })
    }

    pub fn transfer(&self, id: &str) -> Result<Option<TransferJob>, MetadataError> {
        self.with_connection(|c| {
            Ok(c.query_row(
                &format!("SELECT {COLUMNS} FROM transfer_jobs WHERE id=?1"),
                [id],
                row_job,
            )
            .optional()?)
        })
    }
    pub fn transfers(&self, limit: u32, offset: u32) -> Result<Vec<TransferJob>, MetadataError> {
        self.with_connection(|c| {
            Ok(c.prepare(&format!(
                "SELECT {COLUMNS} FROM transfer_jobs ORDER BY sequence DESC LIMIT ?1 OFFSET ?2"
            ))?
            .query_map(params![limit.min(100), offset], row_job)?
            .collect::<Result<Vec<_>, _>>()?)
        })
    }
    pub(crate) fn transfers_for_local_cleanup(
        &self,
        limit: u32,
    ) -> Result<Vec<TransferJob>, MetadataError> {
        self.with_connection(|c| {
            Ok(c.prepare(&format!(
                "SELECT {COLUMNS} FROM transfer_jobs WHERE state IN ('completed','cancelled','reception_failed','superseded') ORDER BY sequence ASC LIMIT ?1"
            ))?
            .query_map([limit.min(100)], row_job)?
            .collect::<Result<Vec<_>, _>>()?)
        })
    }
    pub(crate) fn claim_transfer(&self) -> Result<Option<TransferJob>, MetadataError> {
        self.with_connection(|c| {
            let tx = c.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
            tx.execute("UPDATE transfer_jobs SET state='recovery_required',error='Reception interrupted; upload the source file again' WHERE state='receiving' AND lease_until < ?1",[now()])?;
            // A send that may have reached Telegram but was not checkpointed is
            // deliberately not replayed. Retain it for operator reconciliation.
            tx.execute(
                "UPDATE transfer_jobs SET state='recovery_required',lease=NULL,lease_until=0,error='Telegram acknowledgement is unknown; automatic reconciliation is pending' WHERE state IN ('uploading','committing') AND lease_until<?1 AND EXISTS(SELECT 1 FROM transfer_send_attempts a WHERE a.job_id=transfer_jobs.id AND a.state IN ('sending','unknown'))",
                [now()],
            )?;
            // A worker can lose its lease after it has already moved the job to
            // recovery_required. In that case the job no longer has an active
            // lease, so the normal uploading/committing transition above will
            // not run on the next process start. Normalize the durable send
            // attempt independently of the old lease so reconciliation can
            // make progress instead of leaving a permanent `sending` row.
            tx.execute(
                "UPDATE transfer_send_attempts SET state='unknown',error_kind='lease_expired',error='Worker lease expired before Telegram acknowledgement',finished_at=?1 WHERE state='sending' AND EXISTS (SELECT 1 FROM transfer_jobs WHERE transfer_jobs.id=transfer_send_attempts.job_id AND ((transfer_jobs.state='recovery_required' AND transfer_jobs.lease IS NULL) OR (transfer_jobs.state IN ('uploading','committing') AND transfer_jobs.lease_until<?1)))",
                [now()],
            )?;
            tx.execute(
                "UPDATE transfer_jobs SET state='cancelled',lease=NULL,lease_until=0,error='Multipart session closed before transfer publication' WHERE id IN (SELECT p.job_id FROM multipart_jobs p JOIN multipart_uploads u ON u.upload_id=p.upload_id WHERE u.state NOT IN ('initiated','uploading')) AND state IN ('queued','retry_wait','recovery_required')",
                [],
            )?;
            // Defensive migration rule for older draft data: an older retry can
            // never replace a newer already-committed write for the same key.
            tx.execute(
                "UPDATE transfer_jobs AS old SET state='superseded',lease=NULL,lease_until=0,error='Superseded by a newer committed write' WHERE old.state IN ('queued','retry_wait','uploading','committing','recovery_required') AND EXISTS(SELECT 1 FROM transfer_jobs newer WHERE newer.bucket=old.bucket AND newer.object_key=old.object_key AND newer.sequence>old.sequence AND newer.state IN ('completed','cleaned'))",
                [],
            )?;
            let id: Option<String> = tx.query_row(
                "SELECT candidate.id FROM transfer_jobs candidate WHERE (((candidate.state IN ('queued','retry_wait') AND candidate.next_retry<=?1) OR (candidate.state IN ('uploading','committing') AND candidate.lease_until<?1)) AND NOT EXISTS(SELECT 1 FROM transfer_send_attempts a WHERE a.job_id=candidate.id AND a.state IN ('sending','unknown')) AND NOT EXISTS(SELECT 1 FROM transfer_jobs older WHERE older.bucket=candidate.bucket AND older.object_key=candidate.object_key AND older.sequence<candidate.sequence AND older.state NOT IN ('completed','cleaned','cancelled','reception_failed','superseded'))) ORDER BY candidate.sequence LIMIT 1",
                [now()],
                |r| r.get(0),
            ).optional()?;
            let Some(id) = id else {
                // The normalization above is itself durable recovery work.
                // Commit it even when there is no newly claimable transfer;
                // otherwise a recovery-required job with no active lease
                // rolls back to the stuck `sending` state on every poll.
                tx.commit()?;
                return Ok(None);
            };
            let lease = Uuid::new_v4().to_string();
            tx.execute("UPDATE transfer_jobs SET state='uploading',lease=?2,lease_until=?3+120,attempts=attempts+1,updated_at=?3 WHERE id=?1",params![id,lease,now()])?;
            let job = tx.query_row(&format!("SELECT {COLUMNS} FROM transfer_jobs WHERE id=?1"),[id],row_job)?;
            tx.commit()?;
            Ok(Some(job))
        })
    }
    pub(crate) fn renew_transfer(&self, id: &str, lease: &str) -> Result<bool, MetadataError> {
        self.with_connection(|c| Ok(c.execute("UPDATE transfer_jobs SET lease_until=?3+120,updated_at=?3 WHERE id=?1 AND lease=?2 AND state IN ('uploading','committing')",params![id,lease,now()])? == 1))
    }

    pub(crate) fn begin_send_attempt(
        &self,
        id: &str,
        lease: &str,
        order: u32,
    ) -> Result<SendAttempt, MetadataError> {
        let attempt = Uuid::new_v4().to_string();
        let token = format!("telegram-s3-{id}-{order}-{attempt}");
        let started_at = now();
        self.with_connection(|c| {
            let tx = c.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
            let valid: bool = tx.query_row(
                "SELECT EXISTS(SELECT 1 FROM transfer_jobs WHERE id=?1 AND lease=?2 AND state IN ('uploading','committing') AND lease_until>=?3)",
                params![id, lease, now()],
                |r| r.get(0),
            )?;
            if !valid {
                return Err(MetadataError::InvalidManifest("transfer lease lost".into()));
            }
            tx.execute(
                "INSERT INTO transfer_send_attempts(job_id,chunk_order,attempt_id,attempt_token,state,started_at) VALUES (?1,?2,?3,?4,'sending',?5)",
                params![id, order, attempt, token, started_at],
            )?;
            tx.commit()?;
            Ok(())
        })?;
        Ok(SendAttempt {
            id: attempt,
            token,
            started_at,
        })
    }

    pub(crate) fn finish_send_attempt(
        &self,
        id: &str,
        lease: &str,
        order: u32,
        attempt: &str,
        location: &TelegramLocation,
    ) -> Result<(), MetadataError> {
        self.with_connection(|c| {
            let tx = c.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
            let valid: bool = tx.query_row(
                "SELECT EXISTS(SELECT 1 FROM transfer_jobs WHERE id=?1 AND lease=?2 AND state IN ('uploading','committing') AND lease_until>=?3)",
                params![id, lease, now()],
                |r| r.get(0),
            )?;
            if !valid {
                return Err(MetadataError::InvalidManifest("transfer lease lost".into()));
            }
            if tx.execute(
                "UPDATE transfer_send_attempts SET state='checkpointed',location_json=?4,finished_at=?5,error_kind=NULL,error=NULL,retryable=0 WHERE job_id=?1 AND chunk_order=?2 AND attempt_id=?3 AND state='sending'",
                params![id, order, attempt, serde_json::to_string(location)?, now()],
            )? != 1 {
                return Err(MetadataError::InvalidManifest("send attempt fence lost".into()));
            }
            tx.execute(
                "INSERT INTO transfer_chunks(job_id,chunk_order,location_json) VALUES (?1,?2,?3) ON CONFLICT(job_id,chunk_order) DO UPDATE SET location_json=excluded.location_json",
                params![id, order, serde_json::to_string(location)?],
            )?;
            tx.execute(
                "UPDATE transfer_jobs SET chunks_done=(SELECT COUNT(*) FROM transfer_chunks WHERE job_id=?1 AND chunk_order<4294967295),updated_at=?2 WHERE id=?1",
                params![id, now()],
            )?;
            tx.commit()?;
            Ok(())
        })
    }

    pub(crate) fn mark_send_attempt_unknown(
        &self,
        id: &str,
        lease: &str,
        order: u32,
        attempt: &str,
        error_kind: &str,
        error: &str,
    ) -> Result<(), MetadataError> {
        self.with_connection(|c| {
            let changed = c.execute(
                "UPDATE transfer_send_attempts SET state='unknown',error_kind=?4,error=?5,finished_at=?6,retryable=0 WHERE job_id=?1 AND chunk_order=?2 AND attempt_id=?3 AND state='sending' AND EXISTS(SELECT 1 FROM transfer_jobs WHERE id=?1 AND lease=?7 AND state IN ('uploading','committing'))",
                params![id, order, attempt, error_kind, error, now(), lease],
            )?;
            if changed != 1 {
                return Err(MetadataError::InvalidManifest(
                    "send attempt could not be marked unknown".into(),
                ));
            }
            Ok(())
        })
    }

    pub(crate) fn unresolved_send_attempts(
        &self,
        id: &str,
    ) -> Result<Vec<UnresolvedSendAttempt>, MetadataError> {
        self.with_connection(|c| {
            let mut statement = c.prepare(
                "SELECT job_id,chunk_order,attempt_id,attempt_token,started_at FROM transfer_send_attempts WHERE job_id=?1 AND state='unknown' ORDER BY chunk_order",
            )?;
            Ok(statement
                .query_map([id], |row| {
                    Ok(UnresolvedSendAttempt {
                        job_id: row.get(0)?,
                        order: row.get(1)?,
                        id: row.get(2)?,
                        token: row.get(3)?,
                        started_at: row.get(4)?,
                    })
                })?
                .collect::<Result<Vec<_>, _>>()?)
        })
    }

    pub(crate) fn resolve_send_attempt(
        &self,
        attempt: &UnresolvedSendAttempt,
        location: Option<&TelegramLocation>,
        error: Option<&str>,
    ) -> Result<(), MetadataError> {
        self.with_connection(|c| {
            let tx = c.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
            let (state, location_json, retryable) = if let Some(location) = location {
                ("checkpointed", Some(serde_json::to_string(location)?), 0)
            } else {
                ("retryable", None, 1)
            };
            if tx.execute(
                "UPDATE transfer_send_attempts SET state=?4,location_json=COALESCE(?5,location_json),error_kind=CASE WHEN ?4='retryable' THEN 'reconciliation_absent' ELSE NULL END,error=?6,finished_at=?7,retryable=?8 WHERE job_id=?1 AND chunk_order=?2 AND attempt_id=?3 AND state='unknown'",
                params![attempt.job_id, attempt.order, attempt.id, state, location_json, error, now(), retryable],
            )? != 1 {
                return Err(MetadataError::InvalidManifest("send attempt resolution fence lost".into()));
            }
            if let Some(location) = location {
                tx.execute(
                    "INSERT INTO transfer_chunks(job_id,chunk_order,location_json) VALUES (?1,?2,?3) ON CONFLICT(job_id,chunk_order) DO UPDATE SET location_json=excluded.location_json",
                    params![attempt.job_id, attempt.order, serde_json::to_string(location)?],
                )?;
            }
            tx.execute(
                "UPDATE transfer_jobs SET chunks_done=(SELECT COUNT(*) FROM transfer_chunks WHERE job_id=?1 AND chunk_order<4294967295),updated_at=?2 WHERE id=?1",
                params![attempt.job_id, now()],
            )?;
            tx.commit()?;
            Ok(())
        })
    }

    pub(crate) fn queue_after_reconciliation(&self, id: &str) -> Result<bool, MetadataError> {
        self.with_connection(|c| {
            Ok(c.execute(
                "UPDATE transfer_jobs SET state='queued',error='Automatic reconciliation completed; retry scheduled',next_retry=0,updated_at=?2 WHERE id=?1 AND state='recovery_required' AND NOT EXISTS(SELECT 1 FROM transfer_send_attempts WHERE job_id=?1 AND state IN ('sending','unknown'))",
                params![id, now()],
            )? == 1)
        })
    }

    pub(crate) fn set_transfer_error(
        &self,
        id: &str,
        message: &str,
    ) -> Result<bool, MetadataError> {
        self.with_connection(|c| {
            Ok(c.execute(
                "UPDATE transfer_jobs SET error=?,updated_at=?2 WHERE id=?1 AND state='recovery_required'",
                params![id, message, now()],
            )? == 1)
        })
    }
    pub(crate) fn checkpoint_location(
        &self,
        id: &str,
        order: u32,
    ) -> Result<Option<TelegramLocation>, MetadataError> {
        self.with_connection(|c| {
            let json: Option<String> = c
                .query_row(
                    "SELECT location_json FROM transfer_chunks WHERE job_id=?1 AND chunk_order=?2",
                    params![id, order],
                    |r| r.get(0),
                )
                .optional()?;
            json.map(|s| serde_json::from_str(&s).map_err(MetadataError::from))
                .transpose()
        })
    }
    pub(crate) fn transfer_failed(
        &self,
        id: &str,
        lease: &str,
        retry_after: Option<u64>,
        message: &str,
    ) -> Result<(), MetadataError> {
        self.with_connection(|c| {
            let ambiguous: bool = c.query_row(
                "SELECT EXISTS(SELECT 1 FROM transfer_send_attempts WHERE job_id=?1 AND state IN ('sending','unknown'))",
                [id],
                |r| r.get(0),
            )?;
            let state = if ambiguous { "recovery_required" } else if retry_after.is_some() { "retry_wait" } else { "recovery_required" };
            let error = if ambiguous { "Telegram acknowledgement is unknown; automatic reconciliation is pending" } else { message };
            c.execute("UPDATE transfer_jobs SET state=?3,error=?4,next_retry=?5,lease=NULL,lease_until=0,updated_at=?6 WHERE id=?1 AND lease=?2 AND state IN ('uploading','committing')",params![id,lease,state,error,now().saturating_add(retry_after.unwrap_or(0) as i64),now()])?;
            Ok(())
        })
    }
    pub fn transfer_action(&self, id: &str, action: &str) -> Result<bool, MetadataError> {
        self.with_connection(|c| {
            if action == "retry" {
                return Ok(c.execute(
                    "UPDATE transfer_jobs SET state='queued',error=NULL,next_retry=0 WHERE id=?1 AND state IN ('retry_wait','recovery_required') AND operation_id IS NOT NULL AND NOT EXISTS(SELECT 1 FROM transfer_send_attempts WHERE job_id=?1 AND state IN ('sending','unknown'))",
                    [id],
                )? == 1);
            }
            if action != "cancel" {
                return Ok(false);
            }
            let tx = c.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
            let object_id: Option<String> = tx.query_row(
                "SELECT object_id FROM transfer_jobs WHERE id=?1 AND state IN ('queued','retry_wait','recovery_required')",
                [id],
                |r| r.get(0),
            ).optional()?;
            let Some(object_id) = object_id else { return Ok(false); };
            let connection_id = active_connection_id(&tx)?;
            let locations = {
                let mut statement = tx.prepare("SELECT location_json FROM transfer_chunks WHERE job_id=?1")?;
                statement.query_map([id], |r| r.get::<_, String>(0))?
                    .collect::<Result<Vec<_>, _>>()?
            };
            if !locations.is_empty() {
                tx.execute(
                    "INSERT OR IGNORE INTO cleanup_targets(object_id,connection_id,peer_id,message_id,target_kind,due_at) VALUES (?1,?2,?3,0,'evidence',?4)",
                    params![object_id, connection_id, format!("evidence:{object_id}"), now()],
                )?;
                for json in locations {
                    let location: TelegramLocation = serde_json::from_str(&json)?;
                    tx.execute(
                        "INSERT OR IGNORE INTO cleanup_targets(object_id,connection_id,peer_id,message_id,target_kind,due_at) VALUES (?1,?2,?3,?4,'message',?5)",
                        params![object_id, connection_id, location.peer_id, location.message_id, now()],
                    )?;
                }
            }
            let changed = tx.execute(
                "UPDATE transfer_jobs SET state='cancelled',error='Cancelled; staged data queued for local discard',lease=NULL,lease_until=0,updated_at=?2 WHERE id=?1 AND state IN ('queued','retry_wait','recovery_required')",
                params![id, now()],
            )? == 1;
            tx.commit()?;
            Ok(changed)
        })
    }

    pub(crate) fn allocate_mock_message_id(&self, observed_max: i32) -> Result<i32, MetadataError> {
        self.with_connection(|c| {
            let tx = c.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
            let stored: i64 = tx.query_row(
                "SELECT value FROM app_settings WHERE key='mock_message_counter'",
                [],
                |r| r.get::<_, String>(0),
            ).optional()?.and_then(|v| v.parse().ok()).unwrap_or(0);
            let next = stored.max(i64::from(observed_max)).checked_add(1)
                .ok_or_else(|| MetadataError::InvalidManifest("mock message id exhausted".into()))?;
            if next > i64::from(i32::MAX) {
                return Err(MetadataError::InvalidManifest("mock message id exhausted".into()));
            }
            tx.execute(
                "INSERT INTO app_settings(key,value,updated_at) VALUES ('mock_message_counter',?1,?2) ON CONFLICT(key) DO UPDATE SET value=excluded.value,updated_at=excluded.updated_at",
                params![next.to_string(), now().to_string()],
            )?;
            tx.commit()?;
            Ok(next as i32)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::manifest::{CommittedManifestArgs, ObjectManifest, TelegramLocation};
    use crate::metadata::BucketRecord;

    fn store() -> MetadataStore {
        let s = MetadataStore::open_in_memory().unwrap();
        s.create_bucket(BucketRecord {
            name: "test".into(),
            created_at: OffsetDateTime::now_utc(),
            deleted_at: None,
            versioning_enabled: false,
            object_locking_enabled: false,
        })
        .unwrap();
        s
    }
    #[test]
    fn budget_claim_and_fencing() {
        let s = store();
        let id = s.begin_transfer(Uuid::new_v4(), "test", "key").unwrap();
        s.reserve_staging(&id, 8, 10).unwrap();
        assert!(s.reserve_staging(&id, 3, 10).is_err());
        s.queue_transfer(&id, Uuid::new_v4(), 1).unwrap();
        let j = s.claim_transfer().unwrap().unwrap();
        assert!(s.claim_transfer().unwrap().is_none());
        assert!(!s.renew_transfer(&id, "stale").unwrap());
        assert!(s.renew_transfer(&id, j.lease.as_deref().unwrap()).unwrap());
    }

    #[test]
    fn ambiguous_send_is_durable_and_can_be_reconciled() {
        let s = store();
        let id = s
            .begin_transfer(Uuid::new_v4(), "test", "ambiguous.bin")
            .unwrap();
        s.reserve_staging(&id, 8, 8).unwrap();
        s.queue_transfer(&id, Uuid::new_v4(), 1).unwrap();
        let job = s.claim_transfer().unwrap().unwrap();
        let lease = job.lease.as_deref().unwrap();
        let attempt = s.begin_send_attempt(&id, lease, 0).unwrap();
        assert!(attempt.token.contains(&id));
        s.mark_send_attempt_unknown(
            &id,
            lease,
            0,
            &attempt.id,
            "telegram_send",
            "timeout after upload",
        )
        .unwrap();
        let unresolved = s.unresolved_send_attempts(&id).unwrap();
        assert_eq!(unresolved.len(), 1);
        assert_eq!(unresolved[0].token, attempt.token);
        s.transfer_failed(&id, lease, Some(1), "telegram send failed")
            .unwrap();
        s.resolve_send_attempt(&unresolved[0], None, Some("not found"))
            .unwrap();
        assert!(s.queue_after_reconciliation(&id).unwrap());
        assert_eq!(s.transfer(&id).unwrap().unwrap().state, "queued");
        let state: (String, String, String, i64) = s
            .with_connection(|c| {
                Ok(c.query_row(
                    "SELECT state,error_kind,error,retryable FROM transfer_send_attempts WHERE job_id=?1",
                    [&id],
                    |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
                )?)
            })
            .unwrap();
        assert_eq!(
            state,
            (
                "retryable".into(),
                "reconciliation_absent".into(),
                "not found".into(),
                1
            )
        );
    }

    #[test]
    fn recovery_required_without_lease_normalizes_inflight_send_for_reconciliation() {
        let s = store();
        let id = s
            .begin_transfer(Uuid::new_v4(), "test", "stuck.bin")
            .unwrap();
        s.reserve_staging(&id, 8, 8).unwrap();
        s.queue_transfer(&id, Uuid::new_v4(), 1).unwrap();
        let job = s.claim_transfer().unwrap().unwrap();
        let lease = job.lease.as_deref().unwrap();
        let attempt = s.begin_send_attempt(&id, lease, 237).unwrap();

        // Reproduce the persisted production state: the worker has already
        // fenced the job as recovery-required, but the send row remained in
        // `sending` because the lease-fenced transition could not run.
        s.with_connection(|connection| {
            connection.execute(
                "UPDATE transfer_jobs SET state='recovery_required',lease=NULL,lease_until=0,error='Telegram acknowledgement is unknown; automatic reconciliation is pending' WHERE id=?1",
                [&id],
            )?;
            Ok(())
        })
        .unwrap();

        let before: (String, Option<String>, String) = s
            .with_connection(|connection| {
                Ok(connection.query_row(
                    "SELECT j.state,j.lease,a.state FROM transfer_jobs j JOIN transfer_send_attempts a ON a.job_id=j.id WHERE j.id=?1",
                    [&id],
                    |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
                )?)
            })
            .unwrap();
        assert_eq!(before, ("recovery_required".into(), None, "sending".into()));

        assert!(s.claim_transfer().unwrap().is_none());
        let after: (String, Option<String>, String) = s
            .with_connection(|connection| {
                Ok(connection.query_row(
                    "SELECT j.state,j.lease,a.state FROM transfer_jobs j JOIN transfer_send_attempts a ON a.job_id=j.id WHERE j.id=?1",
                    [&id],
                    |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
                )?)
            })
            .unwrap();
        assert_eq!(after, ("recovery_required".into(), None, "unknown".into()));
        let unresolved = s.unresolved_send_attempts(&id).unwrap();
        assert_eq!(unresolved.len(), 1);
        assert_eq!(unresolved[0].order, 237);
        assert_eq!(unresolved[0].token, attempt.token);

        s.resolve_send_attempt(&unresolved[0], None, Some("remote document absent"))
            .unwrap();
        assert!(s.queue_after_reconciliation(&id).unwrap());
        let repaired = s.transfer(&id).unwrap().unwrap();
        assert_eq!(repaired.state, "queued");
        assert_eq!(repaired.chunks_done, 0);
    }

    #[test]
    fn retry_action_is_not_permanently_blocked_by_stale_sending_row() {
        let s = store();
        let id = s
            .begin_transfer(Uuid::new_v4(), "test", "retry.bin")
            .unwrap();
        s.reserve_staging(&id, 8, 8).unwrap();
        s.queue_transfer(&id, Uuid::new_v4(), 1).unwrap();
        let job = s.claim_transfer().unwrap().unwrap();
        let lease = job.lease.as_deref().unwrap();
        let attempt = s.begin_send_attempt(&id, lease, 0).unwrap();
        s.with_connection(|connection| {
            connection.execute(
                "UPDATE transfer_jobs SET state='recovery_required',lease=NULL,lease_until=0,error='Telegram acknowledgement is unknown; automatic reconciliation is pending',operation_id=?2 WHERE id=?1",
                rusqlite::params![id, Uuid::new_v4().to_string()],
            )?;
            Ok(())
        })
        .unwrap();

        assert!(!s.transfer_action(&id, "retry").unwrap());
        assert!(s.claim_transfer().unwrap().is_none());
        let unresolved = s.unresolved_send_attempts(&id).unwrap();
        assert_eq!(unresolved[0].id, attempt.id);
        s.resolve_send_attempt(&unresolved[0], None, Some("remote document absent"))
            .unwrap();
        assert!(s.queue_after_reconciliation(&id).unwrap());
        assert_eq!(s.transfer(&id).unwrap().unwrap().state, "queued");
    }

    #[test]
    fn durable_metrics_report_live_backlog() {
        let s = store();
        let transfer_id = s
            .begin_transfer(Uuid::new_v4(), "test", "pending.txt")
            .unwrap();
        s.reserve_staging(&transfer_id, 8, 100).unwrap();

        let manifest = ObjectManifest::committed(CommittedManifestArgs {
            bucket: "test".into(),
            key: "cleanup.txt".into(),
            content_length: 0,
            content_type: "text/plain".into(),
            checksum_algorithm: "sha256".into(),
            whole_object: "abcd".into(),
            peer_id: "peer".into(),
            message_id: 0,
        });
        let op = s
            .stage_manifest(crate::metadata::OperationKind::Put, manifest.clone())
            .unwrap();
        s.commit_manifest(op).unwrap();
        s.tombstone_manifest(manifest.object_id, "metrics test")
            .unwrap();

        let metrics = s.durable_metrics().unwrap();
        assert_eq!(metrics.pending_jobs, 1);
        assert_eq!(metrics.staging_bytes, 8);
        assert!(metrics.cleanup_backlog > 0);
        assert_eq!(metrics.cleanup_recovery_required, 0);
        assert_eq!(metrics.failed_jobs, 0);
    }

    #[test]
    fn cleanup_outbox_follows_evidence_and_retry_state_machine() {
        let s = store();
        s.set_telegram_bootstrap_settings(&crate::metadata::TelegramBootstrapSettings {
            telegram_api_id: Some("123".into()),
            telegram_api_hash: Some("hash".into()),
            telegram_storage_chat_id: Some("-1001234567890".into()),
            ..Default::default()
        })
        .unwrap();
        let manifest = ObjectManifest::committed(CommittedManifestArgs {
            bucket: "test".into(),
            key: "key".into(),
            content_length: 0,
            content_type: "text/plain".into(),
            checksum_algorithm: "sha256".into(),
            whole_object: "abcd".into(),
            peer_id: "peer".into(),
            message_id: 0,
        });
        let op = s
            .stage_manifest(crate::metadata::OperationKind::Put, manifest.clone())
            .unwrap();
        s.commit_manifest(op).unwrap();
        s.tombstone_manifest(manifest.object_id, "cleanup test")
            .unwrap();

        s.make_cleanup_due(&manifest.object_id.to_string()).unwrap();

        let evidence = s.claim_cleanup().unwrap().expect("evidence target");
        assert_eq!(evidence.kind, "evidence");
        assert_ne!(evidence.connection_id, "legacy");
        s.complete_cleanup(
            evidence.id,
            &evidence.lease,
            Some(&TelegramLocation {
                peer_id: evidence.peer_id.clone(),
                message_id: evidence.message_id,
                document_id: None,
            }),
        )
        .unwrap();
        assert!(
            s.cleanup_complete_for_object(&manifest.object_id.to_string())
                .unwrap()
        );

        s.with_connection(|c| {
            c.execute(
                "INSERT INTO cleanup_targets(object_id,peer_id,message_id,target_kind,due_at) VALUES (?1,?2,?3,'message',?4)",
                params![manifest.object_id.to_string(), "peer", 77, now() - 1],
            )?;
            Ok(())
        })
        .unwrap();

        let first_message = s.claim_cleanup().unwrap().expect("first message");
        assert_eq!(first_message.kind, "message");
        s.fail_cleanup(
            first_message.id,
            &first_message.lease,
            60,
            "temporary cleanup failure",
        )
        .unwrap();
        assert!(s.claim_cleanup().unwrap().is_none());
        s.make_cleanup_due(&manifest.object_id.to_string()).unwrap();
        let retried_message = s.claim_cleanup().unwrap().expect("retried message");
        assert_eq!(retried_message.id, first_message.id);
        s.complete_cleanup(retried_message.id, &retried_message.lease, None)
            .unwrap();

        s.with_connection(|c| {
            c.execute(
                "INSERT INTO cleanup_targets(object_id,peer_id,message_id,target_kind,due_at) VALUES (?1,?2,?3,'message',?4)",
                params![manifest.object_id.to_string(), "peer", 78, now() - 1],
            )?;
            Ok(())
        })
        .unwrap();

        let second_message = s.claim_cleanup().unwrap().expect("second message");
        assert_eq!(second_message.kind, "message");
        s.quarantine_cleanup(
            second_message.id,
            &second_message.lease,
            "cleanup needs operator review",
        )
        .unwrap();

        assert!(
            !s.cleanup_complete_for_object(&manifest.object_id.to_string())
                .unwrap()
        );
    }

    #[test]
    fn setup_closes_atomically_and_stays_closed() {
        let s = store();
        assert!(s.setup_required().unwrap());
        let u = s
            .create_first_user("operator", "test-hash", "Operator")
            .unwrap();
        assert_eq!(u.role, "superadmin");
        assert!(!s.setup_required().unwrap());
        assert!(s.create_first_user("intruder", "hash", "").is_err());
        assert!(s.delete_user(&u.id).is_err());
        assert!(!s.setup_required().unwrap());
    }
}
