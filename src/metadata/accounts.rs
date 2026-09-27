use super::{MetadataError, MetadataStore, TelegramBootstrapSettings};
use rusqlite::{OptionalExtension, params};
use serde::Serialize;
use uuid::Uuid;

#[derive(Debug, Clone, Serialize)]
pub struct AccountRecord {
    pub id: String,
    pub label: String,
    pub phone: Option<String>,
    pub state: String,
    pub storage_chat_id: Option<String>,
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

#[derive(Debug, Clone, Serialize)]
pub struct RechunkJob {
    pub id: String,
    pub bucket: String,
    pub key: String,
    pub object_id: String,
    pub new_chunk_size: u64,
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

fn account_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<AccountRecord> {
    Ok(AccountRecord {
        id: row.get(0)?,
        label: row.get(1)?,
        phone: row.get(2)?,
        state: row.get(3)?,
        storage_chat_id: row.get(4)?,
        replica_objects: row.get(5)?,
        access_objects: row.get(6)?,
        created_at: row.get(7)?,
        updated_at: row.get(8)?,
    })
}

impl MetadataStore {
    pub fn list_telegram_accounts(&self) -> Result<Vec<AccountRecord>, MetadataError> {
        self.with_connection(|connection| {
            let mut statement = connection.prepare(
                r#"SELECT a.id,a.label,a.phone,a.state,
                    json_extract(a.bootstrap_json,'$.telegram_storage_chat_id'),
                    COUNT(DISTINCT CASE WHEN r.mode='replica' THEN r.object_id END),
                    COUNT(DISTINCT CASE WHEN r.mode='access' THEN r.object_id END),
                    a.created_at,a.updated_at
                   FROM telegram_accounts a
                   LEFT JOIN replica_locations r ON r.account_id=a.id AND r.state='ready'
                   GROUP BY a.id ORDER BY a.created_at ASC"#,
            )?;
            let rows = statement.query_map([], account_from_row)?;
            rows.collect::<Result<Vec<_>, _>>()
                .map_err(MetadataError::from)
        })
    }

    pub fn telegram_account(
        &self,
        id: &str,
    ) -> Result<Option<(AccountRecord, TelegramBootstrapSettings)>, MetadataError> {
        self.with_connection(|connection| {
            let row = connection
                .query_row(
                    r#"SELECT a.id,a.label,a.phone,a.state,
                        json_extract(a.bootstrap_json,'$.telegram_storage_chat_id'),
                        COUNT(DISTINCT CASE WHEN r.mode='replica' THEN r.object_id END),
                        COUNT(DISTINCT CASE WHEN r.mode='access' THEN r.object_id END),
                        a.created_at,a.updated_at,a.bootstrap_json
                       FROM telegram_accounts a
                       LEFT JOIN replica_locations r ON r.account_id=a.id AND r.state='ready'
                       WHERE a.id=?1 GROUP BY a.id"#,
                    [id],
                    |row| Ok((account_from_row(row)?, row.get::<_, String>(9)?)),
                )
                .optional()?;
            row.map(|(record, json)| Ok((record, parse_bootstrap(json)?)))
                .transpose()
        })
    }

    pub fn upsert_telegram_account(
        &self,
        id: Option<&str>,
        label: &str,
        settings: &TelegramBootstrapSettings,
        phone: Option<&str>,
    ) -> Result<AccountRecord, MetadataError> {
        let id = id
            .map(str::to_owned)
            .unwrap_or_else(|| Uuid::new_v4().to_string());
        let json = serde_json::to_string(settings)?;
        let now = crate::durable::now();
        self.with_connection(|connection| {
            connection.execute(
                r#"INSERT INTO telegram_accounts(id,label,bootstrap_json,phone,state,created_at,updated_at)
                   VALUES(?1,?2,?3,?4,'configured',?5,?5)
                   ON CONFLICT(id) DO UPDATE SET label=excluded.label,bootstrap_json=excluded.bootstrap_json,
                     phone=excluded.phone,state='configured',updated_at=excluded.updated_at"#,
                params![id, label.trim(), json, phone, now],
            )?;
            let record = connection.query_row(
                r#"SELECT a.id,a.label,a.phone,a.state,
                    json_extract(a.bootstrap_json,'$.telegram_storage_chat_id'),0,0,a.created_at,a.updated_at
                   FROM telegram_accounts a WHERE a.id=?1"#,
                [&id], account_from_row,
            )?;
            Ok(record)
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
    ) -> Result<RechunkJob, MetadataError> {
        let now = crate::durable::now();
        self.with_connection(|connection| {
            let tx = connection.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
            let object_id: String = tx.query_row("SELECT object_id FROM active_objects WHERE bucket=?1 AND object_key=?2", params![bucket,key], |row| row.get(0)).optional()?.ok_or_else(|| MetadataError::ManifestNotFound(format!("{bucket}/{key}")))?;
            let id = Uuid::new_v4().to_string();
            tx.execute("INSERT INTO rechunk_jobs(id,bucket,object_key,object_id,new_chunk_size,state,created_at,updated_at) VALUES(?1,?2,?3,?4,?5,'queued',?6,?6)", params![id,bucket,key,object_id,new_chunk_size,now])?;
            tx.execute("INSERT INTO rechunk_locks(object_id,job_id,reason,created_at) VALUES(?1,?2,'object is being re-chunked; try again later',?3)", params![object_id,id,now])?;
            tx.commit()?;
            self.rechunk_job(&id)?.ok_or_else(|| MetadataError::InvalidManifest("rechunk job missing".into()))
        })
    }

    pub fn list_rechunk_jobs(&self) -> Result<Vec<RechunkJob>, MetadataError> {
        self.with_connection(|connection| {
            let mut stmt = connection.prepare("SELECT id,bucket,object_key,object_id,new_chunk_size,state,chunks_total,chunks_done,bytes_done,error,created_at,updated_at FROM rechunk_jobs ORDER BY updated_at DESC")?;
            let rows = stmt.query_map([], row_rechunk_job)?;
            rows.collect::<Result<Vec<_>, _>>().map_err(MetadataError::from)
        })
    }

    pub fn rechunk_job(&self, id: &str) -> Result<Option<RechunkJob>, MetadataError> {
        self.with_connection(|connection| connection.query_row("SELECT id,bucket,object_key,object_id,new_chunk_size,state,chunks_total,chunks_done,bytes_done,error,created_at,updated_at FROM rechunk_jobs WHERE id=?1", [id], row_rechunk_job).optional().map_err(MetadataError::from))
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
            let job = tx.query_row("SELECT id,bucket,object_key,object_id,new_chunk_size,state,chunks_total,chunks_done,bytes_done,error,created_at,updated_at FROM rechunk_jobs WHERE id=?1", [&id], row_rechunk_job)?;
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
        new_chunk_size: row.get(4)?,
        state: row.get(5)?,
        chunks_total: row.get(6)?,
        chunks_done: row.get(7)?,
        bytes_done: row.get(8)?,
        error: row.get(9)?,
        created_at: row.get(10)?,
        updated_at: row.get(11)?,
    })
}
