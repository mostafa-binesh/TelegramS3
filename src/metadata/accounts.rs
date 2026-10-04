use super::{MetadataError, MetadataStore, TelegramBootstrapSettings};
use rusqlite::{OptionalExtension, params};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Debug, Clone, Serialize)]
pub struct AccountRecord {
    pub id: String,
    pub label: String,
    pub phone: Option<String>,
    pub state: String,
    pub download_enabled: bool,
    pub storage_chat_id: Option<String>,
    pub quota_bytes: Option<u64>,
    pub used_bytes: u64,
    pub replica_objects: u64,
    pub access_objects: u64,
    pub created_at: i64,
    pub updated_at: i64,
}

#[derive(Debug, Clone, Serialize)]
pub struct AccountAccess {
    pub object_id: String,
    pub bucket: String,
    pub key: String,
    pub chunk_order: u32,
    pub account_id: String,
    pub account_label: String,
    pub mode: String,
    pub peer_id: String,
    pub message_id: i64,
    pub document_id: Option<String>,
    pub state: String,
    pub error: Option<String>,
    pub updated_at: i64,
}

#[derive(Debug, Clone, Serialize)]
pub struct ReplicationJob {
    pub id: String,
    pub source_account_id: String,
    pub target_account_id: String,
    pub bucket: String,
    pub object_keys: Vec<String>,
    pub mode: String,
    pub access_mode: String,
    pub state: String,
    pub objects_total: u64,
    pub objects_done: u64,
    pub chunks_total: u64,
    pub chunks_done: u64,
    pub bytes_done: u64,
    pub next_run: Option<i64>,
    pub last_run: Option<i64>,
    pub error: Option<String>,
    pub created_at: i64,
    pub updated_at: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RechunkReplicaTarget {
    pub account_id: String,
    pub access_mode: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct RechunkJob {
    pub id: String,
    pub bucket: String,
    pub key: String,
    pub object_id: String,
    pub source_account_id: String,
    pub new_chunk_size: u64,
    pub apply_to_replicas: bool,
    pub replica_targets: Vec<RechunkReplicaTarget>,
    pub state: String,
    pub chunks_total: u64,
    pub chunks_done: u64,
    pub bytes_done: u64,
    pub error: Option<String>,
    pub created_at: i64,
    pub updated_at: i64,
}

fn parse_bootstrap(json: String) -> Result<TelegramBootstrapSettings, MetadataError> {
    Ok(serde_json::from_str(&json)?)
}

fn enforce_quota(
    label: &str,
    quota_bytes: Option<u64>,
    used_bytes: u64,
    additional_bytes: u64,
) -> Result<(), MetadataError> {
    let Some(quota_bytes) = quota_bytes else {
        return Ok(());
    };
    let requested_total = used_bytes.saturating_add(additional_bytes);
    if requested_total > quota_bytes {
        return Err(MetadataError::QuotaExceeded(format!(
            "account {label} has {} used of {} and cannot receive {} more bytes",
            used_bytes, quota_bytes, additional_bytes
        )));
    }
    Ok(())
}

pub(crate) fn enforce_account_quota_in_transaction(
    tx: &rusqlite::Transaction<'_>,
    account_id: &str,
    bucket: &str,
    key: &str,
    content_length: u64,
) -> Result<(), MetadataError> {
    let Some((label, quota, used)) = tx
        .query_row(
            "SELECT a.label,a.quota_bytes,COALESCE((SELECT SUM(CAST(json_extract(m.manifest_json,'$.content_length') AS INTEGER)) FROM active_objects ao JOIN object_manifests m ON m.object_id=ao.object_id WHERE m.connection_id=a.id AND m.commit_state='committed'),0)+COALESCE((SELECT SUM(CAST(json_extract(m.manifest_json, '$.chunks[' || r.chunk_order || '].size') AS INTEGER)) FROM replica_locations r JOIN object_manifests m ON m.object_id=r.object_id JOIN active_objects ao ON ao.object_id=m.object_id WHERE r.account_id=a.id AND r.mode='replica' AND r.state='ready' AND m.commit_state='committed'),0) FROM telegram_accounts a WHERE a.id=?1",
            [account_id],
            |row| Ok((row.get::<_, String>(0)?, row.get::<_, Option<i64>>(1)?, row.get::<_, i64>(2)?)),
        )
        .optional()? else {
        return Ok(());
    };
    let current: u64 = tx
        .query_row(
            "SELECT COALESCE(CAST(json_extract(m.manifest_json,'$.content_length') AS INTEGER),0) FROM active_objects ao JOIN object_manifests m ON m.object_id=ao.object_id WHERE ao.bucket=?1 AND ao.object_key=?2 AND m.connection_id=?3 AND m.commit_state='committed'",
            params![bucket, key, account_id],
            |row| row.get::<_, i64>(0),
        )
        .optional()?
        .unwrap_or(0)
        .max(0) as u64;
    enforce_quota(
        &label,
        quota.map(|value| value.max(0) as u64),
        used.max(0) as u64,
        content_length.saturating_sub(current),
    )
}

fn account_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<AccountRecord> {
    Ok(AccountRecord {
        id: row.get(0)?,
        label: row.get(1)?,
        phone: row.get(2)?,
        state: row.get(3)?,
        download_enabled: row.get::<_, i64>(4)? != 0,
        storage_chat_id: row.get(5)?,
        quota_bytes: row.get::<_, Option<i64>>(6)?.map(|value| value as u64),
        used_bytes: row.get::<_, i64>(7)?.max(0) as u64,
        replica_objects: row.get(8)?,
        access_objects: row.get(9)?,
        created_at: row.get(10)?,
        updated_at: row.get(11)?,
    })
}

const ACCOUNT_SELECT: &str = r#"
    SELECT a.id,a.label,a.phone,a.state,a.download_enabled,
        json_extract(a.bootstrap_json,'$.telegram_storage_chat_id'),
        a.quota_bytes,
        COALESCE((
            SELECT SUM(CAST(json_extract(m.manifest_json,'$.content_length') AS INTEGER))
            FROM active_objects ao
            JOIN object_manifests m ON m.object_id=ao.object_id
            WHERE m.connection_id=a.id AND m.commit_state='committed'
        ),0) + COALESCE((
            SELECT SUM(CAST(json_extract(m.manifest_json, '$.chunks[' || r.chunk_order || '].size') AS INTEGER))
            FROM replica_locations r
            JOIN object_manifests m ON m.object_id=r.object_id
            JOIN active_objects ao ON ao.object_id=m.object_id
            WHERE r.account_id=a.id AND r.mode='replica' AND r.state='ready'
              AND m.commit_state='committed'
        ),0),
        COUNT(DISTINCT CASE WHEN r.mode='replica' THEN r.object_id END),
        COUNT(DISTINCT CASE WHEN r.mode='access' THEN r.object_id END),
        a.created_at,a.updated_at
    FROM telegram_accounts a
    LEFT JOIN replica_locations r ON r.account_id=a.id AND r.state='ready'
"#;

fn load_account(
    connection: &rusqlite::Connection,
    id: &str,
) -> Result<Option<AccountRecord>, rusqlite::Error> {
    connection
        .query_row(
            &format!("{ACCOUNT_SELECT} WHERE a.id=?1 GROUP BY a.id"),
            [id],
            account_from_row,
        )
        .optional()
}

impl MetadataStore {
    pub fn list_telegram_accounts(&self) -> Result<Vec<AccountRecord>, MetadataError> {
        self.with_connection(|connection| {
            let mut statement =
                connection.prepare("SELECT id FROM telegram_accounts ORDER BY created_at ASC")?;
            let ids = statement
                .query_map([], |row| row.get::<_, String>(0))?
                .collect::<Result<Vec<_>, _>>()?;
            ids.iter()
                .map(|id| {
                    load_account(connection, id)?
                        .ok_or_else(|| rusqlite::Error::QueryReturnedNoRows)
                })
                .collect::<Result<Vec<_>, _>>()
                .map_err(MetadataError::from)
        })
    }

    pub fn telegram_account(
        &self,
        id: &str,
    ) -> Result<Option<(AccountRecord, TelegramBootstrapSettings)>, MetadataError> {
        self.with_connection(|connection| {
            let json: Option<String> = connection
                .query_row(
                    "SELECT bootstrap_json FROM telegram_accounts WHERE id=?1",
                    [id],
                    |row| row.get(0),
                )
                .optional()?;
            let Some(json) = json else {
                return Ok(None);
            };
            let record =
                load_account(connection, id)?.ok_or(rusqlite::Error::QueryReturnedNoRows)?;
            Ok(Some((record, parse_bootstrap(json)?)))
        })
    }

    pub fn upsert_telegram_account(
        &self,
        id: Option<&str>,
        label: &str,
        settings: &TelegramBootstrapSettings,
        phone: Option<&str>,
        download_enabled: Option<bool>,
        quota_bytes: Option<Option<u64>>,
    ) -> Result<AccountRecord, MetadataError> {
        let id = id
            .map(str::to_owned)
            .unwrap_or_else(|| Uuid::new_v4().to_string());
        let json = serde_json::to_string(settings)?;
        let quota_bytes = quota_bytes
            .map(|value| {
                value.map(i64::try_from).transpose().map_err(|_| {
                    MetadataError::InvalidManifest("account quota is too large".into())
                })
            })
            .transpose()?;
        let now = crate::durable::now();
        self.with_connection(|connection| {
            connection.execute(
                r#"INSERT INTO telegram_accounts(id,label,bootstrap_json,phone,download_enabled,quota_bytes,state,created_at,updated_at)
                   VALUES(?1,?2,?3,?4,COALESCE(?5,1),?6,'configured',?7,?7)
                   ON CONFLICT(id) DO UPDATE SET label=excluded.label,bootstrap_json=excluded.bootstrap_json,
                     phone=excluded.phone,download_enabled=COALESCE(?5,telegram_accounts.download_enabled),
                     quota_bytes=CASE WHEN ?8 THEN ?6 ELSE telegram_accounts.quota_bytes END,
                     state='configured',updated_at=excluded.updated_at"#,
                params![
                    id,
                    label.trim(),
                    json,
                    phone,
                    download_enabled,
                    quota_bytes.flatten(),
                    now,
                    quota_bytes.is_some(),
                ],
            )?;
            let record = load_account(connection, &id)?.ok_or(rusqlite::Error::QueryReturnedNoRows)?;
            Ok(record)
        })
    }

    /// Ensure a new primary object fits the owning account. Replacing the
    /// active object at the same key only consumes the size delta.
    pub fn ensure_account_quota_for_object(
        &self,
        account_id: &str,
        bucket: &str,
        key: &str,
        content_length: u64,
    ) -> Result<(), MetadataError> {
        self.with_connection(|connection| {
            let tx =
                connection.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
            enforce_account_quota_in_transaction(&tx, account_id, bucket, key, content_length)?;
            tx.commit()?;
            Ok(())
        })
    }

    /// Check capacity before a physical replica is copied to an account.
    pub fn ensure_account_quota_for_additional_bytes(
        &self,
        account_id: &str,
        additional_bytes: u64,
    ) -> Result<(), MetadataError> {
        self.with_connection(|connection| {
            let Some((label, quota, used)) = connection
                .query_row(
                    "SELECT a.label,a.quota_bytes,COALESCE((SELECT SUM(CAST(json_extract(m.manifest_json,'$.content_length') AS INTEGER)) FROM active_objects ao JOIN object_manifests m ON m.object_id=ao.object_id WHERE m.connection_id=a.id AND m.commit_state='committed'),0)+COALESCE((SELECT SUM(CAST(json_extract(m.manifest_json, '$.chunks[' || r.chunk_order || '].size') AS INTEGER)) FROM replica_locations r JOIN object_manifests m ON m.object_id=r.object_id JOIN active_objects ao ON ao.object_id=m.object_id WHERE r.account_id=a.id AND r.mode='replica' AND r.state='ready' AND m.commit_state='committed'),0) FROM telegram_accounts a WHERE a.id=?1",
                    [account_id],
                    |row| Ok((row.get::<_, String>(0)?, row.get::<_, Option<i64>>(1)?, row.get::<_, i64>(2)?)),
                )
                .optional()? else {
                    return Ok(());
                };
            enforce_quota(
                &label,
                quota.map(|value| value.max(0) as u64),
                used.max(0) as u64,
                additional_bytes,
            )
        })
    }

    pub fn delete_telegram_account(&self, id: &str) -> Result<bool, MetadataError> {
        self.with_connection(|connection| {
            let active: Option<String> = connection
                .query_row(
                    "SELECT value FROM app_settings WHERE key='telegram_active_connection_id'",
                    [],
                    |row| row.get(0),
                )
                .optional()?;
            let active = active.as_deref() == Some(id);
            if active {
                return Err(MetadataError::InvalidManifest(
                    "the active account cannot be removed; connect another account first".into(),
                ));
            }
            Ok(connection.execute("DELETE FROM telegram_accounts WHERE id=?1", [id])? == 1)
        })
    }

    /// Read eligibility is independent from account ownership. Missing account
    /// rows are treated as disabled so a stale replica cannot silently route a
    /// download through an unknown credential.
    pub fn telegram_account_download_enabled(&self, id: &str) -> Result<bool, MetadataError> {
        self.with_connection(|connection| {
            Ok(connection
                .query_row(
                    "SELECT download_enabled FROM telegram_accounts WHERE id=?1",
                    [id],
                    |row| row.get::<_, i64>(0),
                )
                .optional()?
                .is_some_and(|value| value != 0))
        })
    }

    pub fn queue_replication(
        &self,
        source_account_id: &str,
        target_account_id: &str,
        bucket: &str,
        object_keys: &[String],
        mode: &str,
        access_mode: &str,
    ) -> Result<ReplicationJob, MetadataError> {
        if !matches!(mode, "one_time" | "automatic") {
            return Err(MetadataError::InvalidManifest(
                "invalid replication mode".into(),
            ));
        }
        if !matches!(access_mode, "replica" | "access") {
            return Err(MetadataError::InvalidManifest(
                "invalid replication access mode".into(),
            ));
        }
        let id = Uuid::new_v4().to_string();
        let now = crate::durable::now();
        let next_run = (mode == "automatic").then_some(now);
        let object_keys_json = serde_json::to_string(object_keys)?;
        self.with_connection(|connection| {
            connection.execute(
                r#"INSERT INTO replication_jobs(id,source_account_id,target_account_id,bucket,object_keys_json,mode,access_mode,state,next_run,created_at,updated_at)
                   VALUES(?1,?2,?3,?4,?5,?6,?7,'queued',?8,?9,?9)"#,
                params![id,source_account_id,target_account_id,bucket,object_keys_json,mode,access_mode,next_run,now],
            )?;
            load_replication_job(connection, &id)
        })
    }

    pub fn list_replication_jobs(&self) -> Result<Vec<ReplicationJob>, MetadataError> {
        self.with_connection(|connection| {
            let mut stmt = connection.prepare("SELECT id,source_account_id,target_account_id,bucket,object_keys_json,mode,access_mode,state,objects_total,objects_done,chunks_total,chunks_done,bytes_done,next_run,last_run,error,created_at,updated_at FROM replication_jobs ORDER BY updated_at DESC")?;
            let rows = stmt.query_map([], row_replication_job)?;
            rows.collect::<Result<Vec<_>, _>>().map_err(MetadataError::from)
        })
    }

    pub fn list_account_access(
        &self,
        bucket: Option<&str>,
    ) -> Result<Vec<AccountAccess>, MetadataError> {
        self.with_connection(|connection| {
            let sql = "SELECT r.object_id,m.bucket,m.object_key,r.chunk_order,r.account_id,a.label,r.mode,r.peer_id,r.message_id,r.document_id,r.state,r.error,r.updated_at FROM replica_locations r JOIN object_manifests m ON m.object_id=r.object_id JOIN telegram_accounts a ON a.id=r.account_id WHERE (?1 IS NULL OR m.bucket=?1) ORDER BY m.bucket,m.object_key,r.chunk_order,a.label";
            let mut stmt = connection.prepare(sql)?;
            let map = |row: &rusqlite::Row<'_>| Ok(AccountAccess { object_id: row.get(0)?, bucket: row.get(1)?, key: row.get(2)?, chunk_order: row.get(3)?, account_id: row.get(4)?, account_label: row.get(5)?, mode: row.get(6)?, peer_id: row.get(7)?, message_id: row.get(8)?, document_id: row.get(9)?, state: row.get(10)?, error: row.get(11)?, updated_at: row.get(12)? });
            let rows = stmt.query_map(params![bucket], map)?;
            rows.collect::<Result<Vec<_>, _>>().map_err(MetadataError::from)
        })
    }

    pub fn queue_rechunk(
        &self,
        bucket: &str,
        key: &str,
        new_chunk_size: u64,
        apply_to_replicas: bool,
    ) -> Result<RechunkJob, MetadataError> {
        let now = crate::durable::now();
        self.with_connection(|connection| {
            let tx = connection.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
            let (object_id, mut source_account_id): (String, String) = tx.query_row("SELECT ao.object_id,COALESCE(m.connection_id,'legacy') FROM active_objects ao JOIN object_manifests m ON m.object_id=ao.object_id WHERE ao.bucket=?1 AND ao.object_key=?2", params![bucket,key], |row| Ok((row.get(0)?, row.get(1)?))).optional()?.ok_or_else(|| MetadataError::ManifestNotFound(format!("{bucket}/{key}")))?;
            if source_account_id == "legacy" {
                source_account_id = tx.query_row("SELECT value FROM app_settings WHERE key='telegram_active_connection_id'", [], |row| row.get(0)).optional()?.unwrap_or_else(|| "legacy".to_string());
            }
            let replica_targets = if apply_to_replicas {
                let mut statement = tx.prepare("SELECT DISTINCT account_id,mode FROM replica_locations WHERE object_id=?1 AND state='ready' AND mode IN ('replica','access') ORDER BY account_id,mode")?;
                let rows = statement.query_map([&object_id], |row| Ok(RechunkReplicaTarget { account_id: row.get(0)?, access_mode: row.get(1)? }))?;
                rows.collect::<Result<Vec<_>, _>>()?
            } else {
                Vec::new()
            };
            let replica_targets_json = serde_json::to_string(&replica_targets)?;
            let id = Uuid::new_v4().to_string();
            tx.execute("INSERT INTO rechunk_jobs(id,bucket,object_key,object_id,source_account_id,new_chunk_size,apply_to_replicas,replica_targets_json,state,created_at,updated_at) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,'queued',?9,?9)", params![id,bucket,key,object_id,source_account_id,new_chunk_size,apply_to_replicas,replica_targets_json,now])?;
            tx.execute("INSERT INTO rechunk_locks(object_id,job_id,reason,created_at) VALUES(?1,?2,'object is being re-chunked; try again later',?3)", params![object_id,id,now])?;
            tx.commit()?;
            self.rechunk_job(&id)?.ok_or_else(|| MetadataError::InvalidManifest("rechunk job missing".into()))
        })
    }

    pub fn list_rechunk_jobs(&self) -> Result<Vec<RechunkJob>, MetadataError> {
        self.with_connection(|connection| {
            let mut stmt = connection.prepare("SELECT id,bucket,object_key,object_id,source_account_id,new_chunk_size,apply_to_replicas,replica_targets_json,state,chunks_total,chunks_done,bytes_done,error,created_at,updated_at FROM rechunk_jobs ORDER BY updated_at DESC")?;
            let rows = stmt.query_map([], row_rechunk_job)?;
            rows.collect::<Result<Vec<_>, _>>().map_err(MetadataError::from)
        })
    }

    pub fn rechunk_job(&self, id: &str) -> Result<Option<RechunkJob>, MetadataError> {
        self.with_connection(|connection| connection.query_row("SELECT id,bucket,object_key,object_id,source_account_id,new_chunk_size,apply_to_replicas,replica_targets_json,state,chunks_total,chunks_done,bytes_done,error,created_at,updated_at FROM rechunk_jobs WHERE id=?1", [id], row_rechunk_job).optional().map_err(MetadataError::from))
    }

    pub fn object_rechunk_locked(&self, object_id: Uuid) -> Result<bool, MetadataError> {
        self.with_connection(|connection| {
            connection
                .query_row(
                    "SELECT EXISTS(SELECT 1 FROM rechunk_locks WHERE object_id=?1)",
                    [object_id.to_string()],
                    |row| row.get::<_, i64>(0),
                )
                .map(|v| v != 0)
                .map_err(MetadataError::from)
        })
    }

    pub fn claim_rechunk_job(&self) -> Result<Option<RechunkJob>, MetadataError> {
        self.with_connection(|connection| {
            let tx = connection.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
            let id: Option<String> = tx.query_row("SELECT id FROM rechunk_jobs WHERE state='queued' ORDER BY created_at ASC LIMIT 1", [], |row| row.get(0)).optional()?;
            let Some(id) = id else { tx.commit()?; return Ok(None); };
            tx.execute("UPDATE rechunk_jobs SET state='processing',updated_at=?2 WHERE id=?1 AND state='queued'", params![id, crate::durable::now()])?;
            let job = tx.query_row("SELECT id,bucket,object_key,object_id,source_account_id,new_chunk_size,apply_to_replicas,replica_targets_json,state,chunks_total,chunks_done,bytes_done,error,created_at,updated_at FROM rechunk_jobs WHERE id=?1", [&id], row_rechunk_job)?;
            tx.commit()?;
            Ok(Some(job))
        })
    }

    pub fn update_rechunk_progress(
        &self,
        id: &str,
        chunks_total: u64,
        chunks_done: u64,
        bytes_done: u64,
    ) -> Result<(), MetadataError> {
        self.with_connection(|connection| {
            connection.execute("UPDATE rechunk_jobs SET chunks_total=?2,chunks_done=?3,bytes_done=?4,updated_at=?5 WHERE id=?1", params![id,chunks_total,chunks_done,bytes_done,crate::durable::now()])?;
            Ok(())
        })
    }

    pub fn finish_rechunk(
        &self,
        id: &str,
        state: &str,
        error: Option<&str>,
    ) -> Result<(), MetadataError> {
        self.with_connection(|connection| {
            let tx = connection.transaction()?;
            tx.execute(
                "UPDATE rechunk_jobs SET state=?2,error=?3,updated_at=?4 WHERE id=?1",
                params![id, state, error, crate::durable::now()],
            )?;
            tx.execute("DELETE FROM rechunk_locks WHERE job_id=?1", [id])?;
            tx.commit()?;
            Ok(())
        })
    }

    pub fn claim_replication_job(&self) -> Result<Option<ReplicationJob>, MetadataError> {
        self.with_connection(|connection| {
            let tx = connection.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
            let now = crate::durable::now();
            let id: Option<String> = tx.query_row("SELECT id FROM replication_jobs WHERE state='queued' OR (state='scheduled' AND next_run IS NOT NULL AND next_run<=?1) ORDER BY updated_at ASC LIMIT 1", [now], |row| row.get(0)).optional()?;
            let Some(id) = id else { tx.commit()?; return Ok(None); };
            tx.execute("UPDATE replication_jobs SET state='processing',updated_at=?2 WHERE id=?1", params![id,now])?;
            let job = tx.query_row("SELECT id,source_account_id,target_account_id,bucket,object_keys_json,mode,access_mode,state,objects_total,objects_done,chunks_total,chunks_done,bytes_done,next_run,last_run,error,created_at,updated_at FROM replication_jobs WHERE id=?1", [&id], row_replication_job)?;
            tx.commit()?;
            Ok(Some(job))
        })
    }

    #[allow(clippy::too_many_arguments)]
    pub fn insert_replica_location(
        &self,
        object_id: Uuid,
        chunk_order: u32,
        account_id: &str,
        mode: &str,
        peer_id: &str,
        message_id: i64,
        document_id: Option<&str>,
    ) -> Result<(), MetadataError> {
        let now = crate::durable::now();
        self.with_connection(|connection| {
            connection.execute("INSERT INTO replica_locations(object_id,chunk_order,account_id,mode,peer_id,message_id,document_id,state,created_at,updated_at) VALUES(?1,?2,?3,?4,?5,?6,?7,'ready',?8,?8) ON CONFLICT(object_id,chunk_order,account_id) DO UPDATE SET mode=excluded.mode,peer_id=excluded.peer_id,message_id=excluded.message_id,document_id=excluded.document_id,state='ready',error=NULL,updated_at=excluded.updated_at", params![object_id.to_string(),chunk_order,account_id,mode,peer_id,message_id,document_id,now])?;
            Ok(())
        })
    }

    pub fn update_replication_progress(
        &self,
        id: &str,
        objects_total: u64,
        objects_done: u64,
        chunks_total: u64,
        chunks_done: u64,
        bytes_done: u64,
    ) -> Result<(), MetadataError> {
        self.with_connection(|connection| {
            connection.execute("UPDATE replication_jobs SET objects_total=?2,objects_done=?3,chunks_total=?4,chunks_done=?5,bytes_done=?6,updated_at=?7 WHERE id=?1", params![id,objects_total,objects_done,chunks_total,chunks_done,bytes_done,crate::durable::now()])?;
            Ok(())
        })
    }

    pub fn finish_replication(
        &self,
        id: &str,
        state: &str,
        error: Option<&str>,
        automatic: bool,
    ) -> Result<(), MetadataError> {
        self.with_connection(|connection| {
            let next_run = automatic.then_some(crate::durable::now() + 300);
            connection.execute("UPDATE replication_jobs SET state=?2,error=?3,last_run=?4,next_run=?5,updated_at=?4 WHERE id=?1", params![id,state,error,crate::durable::now(),next_run])?;
            Ok(())
        })
    }
}

fn load_replication_job(
    connection: &rusqlite::Connection,
    id: &str,
) -> Result<ReplicationJob, MetadataError> {
    Ok(connection.query_row("SELECT id,source_account_id,target_account_id,bucket,object_keys_json,mode,access_mode,state,objects_total,objects_done,chunks_total,chunks_done,bytes_done,next_run,last_run,error,created_at,updated_at FROM replication_jobs WHERE id=?1", [id], row_replication_job)?)
}

fn row_replication_job(row: &rusqlite::Row<'_>) -> rusqlite::Result<ReplicationJob> {
    Ok(ReplicationJob {
        id: row.get(0)?,
        source_account_id: row.get(1)?,
        target_account_id: row.get(2)?,
        bucket: row.get(3)?,
        object_keys: serde_json::from_str(&row.get::<_, String>(4)?).unwrap_or_default(),
        mode: row.get(5)?,
        access_mode: row.get(6)?,
        state: row.get(7)?,
        objects_total: row.get(8)?,
        objects_done: row.get(9)?,
        chunks_total: row.get(10)?,
        chunks_done: row.get(11)?,
        bytes_done: row.get(12)?,
        next_run: row.get(13)?,
        last_run: row.get(14)?,
        error: row.get(15)?,
        created_at: row.get(16)?,
        updated_at: row.get(17)?,
    })
}

fn row_rechunk_job(row: &rusqlite::Row<'_>) -> rusqlite::Result<RechunkJob> {
    Ok(RechunkJob {
        id: row.get(0)?,
        bucket: row.get(1)?,
        key: row.get(2)?,
        object_id: row.get(3)?,
        source_account_id: row.get(4)?,
        new_chunk_size: row.get(5)?,
        apply_to_replicas: row.get::<_, i64>(6)? != 0,
        replica_targets: serde_json::from_str(&row.get::<_, String>(7)?).unwrap_or_default(),
        state: row.get(8)?,
        chunks_total: row.get(9)?,
        chunks_done: row.get(10)?,
        bytes_done: row.get(11)?,
        error: row.get(12)?,
        created_at: row.get(13)?,
        updated_at: row.get(14)?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::manifest::{CommittedManifestArgs, ObjectManifest};
    use crate::metadata::{BucketRecord, OperationKind, TelegramBootstrapSettings};
    use time::OffsetDateTime;

    #[test]
    fn account_quota_supports_unlimited_and_reports_derived_usage() {
        let store = MetadataStore::open_in_memory().expect("metadata");
        store
            .set_telegram_bootstrap_settings(&TelegramBootstrapSettings::default())
            .expect("bootstrap");
        let account_id = store
            .active_connection_id()
            .expect("active id")
            .expect("account id");
        let account = store
            .upsert_telegram_account(
                Some(&account_id),
                "Primary",
                &TelegramBootstrapSettings::default(),
                None,
                Some(true),
                Some(Some(10)),
            )
            .expect("bounded account");
        assert_eq!(account.quota_bytes, Some(10));
        assert_eq!(account.used_bytes, 0);
        assert!(
            store
                .ensure_account_quota_for_additional_bytes(&account_id, 10)
                .is_ok()
        );
        assert!(matches!(
            store.ensure_account_quota_for_additional_bytes(&account_id, 11),
            Err(MetadataError::QuotaExceeded(_))
        ));

        store
            .create_bucket(BucketRecord {
                name: "quota-test".into(),
                created_at: OffsetDateTime::now_utc(),
                deleted_at: None,
                versioning_enabled: false,
                object_locking_enabled: false,
            })
            .expect("bucket");
        let manifest = ObjectManifest::committed(CommittedManifestArgs {
            bucket: "quota-test".into(),
            key: "one.bin".into(),
            content_length: 7,
            content_type: "application/octet-stream".into(),
            checksum_algorithm: "sha256".into(),
            whole_object: "checksum".into(),
            peer_id: "peer".into(),
            message_id: 1,
        });
        let operation = store
            .stage_manifest(OperationKind::Put, manifest)
            .expect("stage");
        store.commit_manifest(operation).expect("commit");
        let account = store
            .list_telegram_accounts()
            .expect("accounts")
            .into_iter()
            .find(|item| item.id == account_id)
            .expect("account");
        assert_eq!(account.used_bytes, 7);

        let unlimited = store
            .upsert_telegram_account(
                Some(&account_id),
                "Primary",
                &TelegramBootstrapSettings::default(),
                None,
                None,
                Some(None),
            )
            .expect("unlimited account");
        assert_eq!(unlimited.quota_bytes, None);
        assert!(
            store
                .ensure_account_quota_for_additional_bytes(&account_id, u64::MAX)
                .is_ok()
        );
    }
}
