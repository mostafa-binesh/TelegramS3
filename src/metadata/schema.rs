use super::pool::DEFAULT_CONNECTION_POOL_SIZE;
use super::rows::{count_rows, parse_rfc3339_timestamp, timestamp_now};
use super::{MetadataError, MetadataStatus, MetadataStore, SCHEMA_VERSION};
use crate::config::{
    DEFAULT_CLEANUP_RETENTION_SECS, DEFAULT_DOWNLOAD_ACCOUNT_CONNECTIONS,
    DEFAULT_DOWNLOAD_FAILOVER_RETRIES, DEFAULT_DOWNLOAD_PREFETCH_CHUNKS,
    DEFAULT_DOWNLOAD_PREFETCH_MODE, DEFAULT_RECOVERY_VERIFY_STARTUP,
};
use rusqlite::{Connection, OptionalExtension, params};

impl MetadataStore {
    pub fn open(path: impl AsRef<std::path::Path>) -> Result<Self, MetadataError> {
        let path = path.as_ref().to_path_buf();
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        create_private_metadata_file(&path)?;

        let pool = super::pool::ConnectionPool::open_file(&path, DEFAULT_CONNECTION_POOL_SIZE)?;
        let store = Self {
            path: Some(path),
            pool,
        };
        store.with_connection(|connection| {
            apply_migrations(connection)?;
            backfill_cleanup_outbox(connection)?;
            super::recovery::rebuild_index_internal(connection)?;
            Ok(())
        })?;
        Ok(store)
    }

    pub fn open_in_memory() -> Result<Self, MetadataError> {
        let pool = super::pool::ConnectionPool::open_shared_memory(
            format!(
                "file:telegram_s3_metadata_{}?mode=memory&cache=shared",
                uuid::Uuid::new_v4()
            ),
            DEFAULT_CONNECTION_POOL_SIZE,
        )?;
        let store = Self { path: None, pool };
        store.with_connection(|connection| {
            apply_migrations(connection)?;
            backfill_cleanup_outbox(connection)?;
            super::recovery::rebuild_index_internal(connection)?;
            Ok(())
        })?;
        Ok(store)
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
}

fn apply_migrations(connection: &mut Connection) -> Result<(), MetadataError> {
    let current_version = read_schema_version(connection)?;
    if current_version > SCHEMA_VERSION {
        return Err(MetadataError::UnsupportedSchemaVersion(current_version));
    }
    if current_version >= SCHEMA_VERSION {
        ensure_phase10_schema(connection)?;
        ensure_connection_removal_schema(connection)?;
        ensure_share_schema(connection)?;
        ensure_download_prefetch_setting(connection)?;
        ensure_download_prefetch_mode_setting(connection)?;
        ensure_download_account_connections_setting(connection)?;
        ensure_download_failover_setting(connection)?;
        ensure_traffic_totals_schema(connection)?;
        ensure_multi_account_schema(connection)?;
        ensure_account_download_policy(connection)?;
        ensure_account_quota_schema(connection)?;
        ensure_replication_scope_schema(connection)?;
        ensure_rechunk_replica_schema(connection)?;
        ensure_integrity_recovery_events_schema(connection)?;
        ensure_cleanup_retention_setting(connection)?;
        ensure_recovery_verify_startup_setting(connection)?;
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
            connection_id TEXT NOT NULL DEFAULT 'legacy',
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
            connection_id TEXT NOT NULL DEFAULT 'legacy',
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
            connection_id TEXT NOT NULL DEFAULT 'legacy',
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

        CREATE TABLE IF NOT EXISTS app_settings (
            key TEXT PRIMARY KEY,
            value TEXT NOT NULL,
            updated_at TEXT NOT NULL
        );

        CREATE TABLE IF NOT EXISTS traffic_totals (
            id INTEGER PRIMARY KEY CHECK (id = 1),
            client_upload_bytes TEXT NOT NULL DEFAULT '0',
            client_download_bytes TEXT NOT NULL DEFAULT '0',
            telegram_upload_bytes TEXT NOT NULL DEFAULT '0',
            telegram_download_bytes TEXT NOT NULL DEFAULT '0',
            updated_at TEXT NOT NULL
        );

        INSERT OR IGNORE INTO traffic_totals (
            id,
            client_upload_bytes,
            client_download_bytes,
            telegram_upload_bytes,
            telegram_download_bytes,
            updated_at
        ) VALUES (1, '0', '0', '0', '0', '');

        CREATE TABLE IF NOT EXISTS transfer_jobs (
            sequence INTEGER PRIMARY KEY AUTOINCREMENT,
            id TEXT NOT NULL UNIQUE,
            object_id TEXT NOT NULL,
            connection_id TEXT NOT NULL DEFAULT 'legacy',
            operation_id TEXT,
            bucket TEXT NOT NULL,
            object_key TEXT NOT NULL,
            state TEXT NOT NULL DEFAULT 'receiving',
            bytes INTEGER NOT NULL DEFAULT 0,
            chunks_done INTEGER NOT NULL DEFAULT 0,
            chunks_total INTEGER NOT NULL DEFAULT 0,
            attempts INTEGER NOT NULL DEFAULT 0,
            next_retry INTEGER NOT NULL DEFAULT 0,
            lease TEXT,
            lease_until INTEGER NOT NULL DEFAULT 0,
            write_conditionals_json TEXT,
            error TEXT,
            created_at INTEGER NOT NULL,
            updated_at INTEGER NOT NULL
        );
        CREATE INDEX IF NOT EXISTS idx_transfer_due ON transfer_jobs(state, next_retry, sequence);
        CREATE TABLE IF NOT EXISTS transfer_chunks (
            job_id TEXT NOT NULL REFERENCES transfer_jobs(id),
            chunk_order INTEGER NOT NULL,
            location_json TEXT NOT NULL,
            PRIMARY KEY(job_id, chunk_order)
        );
        CREATE TABLE IF NOT EXISTS transfer_send_attempts (
            job_id TEXT NOT NULL REFERENCES transfer_jobs(id),
            chunk_order INTEGER NOT NULL,
            attempt_id TEXT NOT NULL,
            attempt_token TEXT NOT NULL,
            state TEXT NOT NULL,
            started_at INTEGER NOT NULL,
            finished_at INTEGER,
            location_json TEXT,
            error_kind TEXT,
            error TEXT,
            retryable INTEGER NOT NULL DEFAULT 0,
            PRIMARY KEY(job_id, chunk_order, attempt_id)
        );
        CREATE INDEX IF NOT EXISTS idx_transfer_send_attempt_state
            ON transfer_send_attempts(job_id, state);
        CREATE TABLE IF NOT EXISTS multipart_jobs (
            job_id TEXT PRIMARY KEY REFERENCES transfer_jobs(id),
            upload_id TEXT NOT NULL,
            part_number INTEGER NOT NULL,
            expected_checksum TEXT
        );
        CREATE TABLE IF NOT EXISTS cleanup_targets (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            object_id TEXT NOT NULL,
            peer_id TEXT NOT NULL,
            message_id INTEGER NOT NULL,
            connection_id TEXT NOT NULL DEFAULT 'legacy',
            target_kind TEXT NOT NULL DEFAULT 'message',
            due_at INTEGER NOT NULL,
            state TEXT NOT NULL DEFAULT 'pending',
            attempts INTEGER NOT NULL DEFAULT 0,
            next_retry INTEGER NOT NULL DEFAULT 0,
            lease TEXT,
            lease_until INTEGER NOT NULL DEFAULT 0,
            error TEXT,
            evidence_location_json TEXT,
            evidence_attempt_token TEXT,
            evidence_attempt_started_at INTEGER,
            completed INTEGER NOT NULL DEFAULT 0,
            UNIQUE(peer_id, message_id)
        );
        CREATE INDEX IF NOT EXISTS idx_cleanup_due
            ON cleanup_targets(state, due_at, next_retry, id);
        CREATE INDEX IF NOT EXISTS idx_cleanup_claim
            ON cleanup_targets(target_kind, completed, state, due_at, next_retry, id);
        CREATE INDEX IF NOT EXISTS idx_cleanup_object_kind_completed
            ON cleanup_targets(object_id, target_kind, completed);
        INSERT OR IGNORE INTO app_settings(key, value, updated_at)
            SELECT 'setup_complete', CASE WHEN EXISTS(SELECT 1 FROM users) THEN 'true' ELSE 'false' END, '';
        "#,
    )?;
    tx.execute(
        "INSERT OR IGNORE INTO app_settings(key, value, updated_at) VALUES (?1, ?2, ?3)",
        rusqlite::params![
            super::settings::DOWNLOAD_PREFETCH_CHUNKS_SETTING,
            DEFAULT_DOWNLOAD_PREFETCH_CHUNKS.to_string(),
            timestamp_now()?
        ],
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
    ensure_phase10_schema(connection)?;
    ensure_connection_removal_schema(connection)?;
    ensure_share_schema(connection)?;
    ensure_download_prefetch_setting(connection)?;
    ensure_download_prefetch_mode_setting(connection)?;
    ensure_download_account_connections_setting(connection)?;
    ensure_download_failover_setting(connection)?;
    ensure_traffic_totals_schema(connection)?;
    ensure_multi_account_schema(connection)?;
    ensure_account_download_policy(connection)?;
    ensure_account_quota_schema(connection)?;
    ensure_replication_scope_schema(connection)?;
    ensure_rechunk_replica_schema(connection)?;
    ensure_integrity_recovery_events_schema(connection)?;
    ensure_cleanup_retention_setting(connection)?;
    ensure_recovery_verify_startup_setting(connection)?;
    Ok(())
}

fn ensure_integrity_recovery_events_schema(
    connection: &mut Connection,
) -> Result<(), MetadataError> {
    connection.execute_batch(
        r#"
        CREATE TABLE IF NOT EXISTS integrity_recovery_events (
            fingerprint TEXT PRIMARY KEY,
            issue_json TEXT NOT NULL,
            state TEXT NOT NULL,
            created_at INTEGER NOT NULL,
            updated_at INTEGER NOT NULL
        );
        CREATE INDEX IF NOT EXISTS idx_integrity_recovery_events_updated
            ON integrity_recovery_events(updated_at DESC);
        CREATE INDEX IF NOT EXISTS idx_integrity_recovery_events_state
            ON integrity_recovery_events(state, updated_at DESC);
        "#,
    )?;
    Ok(())
}

/// Multi-account metadata is additive. Existing buckets/manifests keep their
/// connection id; the migration only creates the registry and indexes and
/// adopts the current bootstrap as the primary account when available.
fn ensure_multi_account_schema(connection: &mut Connection) -> Result<(), MetadataError> {
    connection.execute_batch(
        r#"
        CREATE TABLE IF NOT EXISTS telegram_accounts (
            id TEXT PRIMARY KEY,
            label TEXT NOT NULL,
            bootstrap_json TEXT NOT NULL,
            phone TEXT,
            download_enabled INTEGER NOT NULL DEFAULT 1,
            state TEXT NOT NULL DEFAULT 'configured',
            created_at INTEGER NOT NULL,
            updated_at INTEGER NOT NULL
        );
        CREATE INDEX IF NOT EXISTS idx_telegram_accounts_state
            ON telegram_accounts(state, updated_at);

        CREATE TABLE IF NOT EXISTS replica_locations (
            object_id TEXT NOT NULL,
            chunk_order INTEGER NOT NULL,
            account_id TEXT NOT NULL,
            mode TEXT NOT NULL DEFAULT 'replica',
            peer_id TEXT NOT NULL,
            message_id INTEGER NOT NULL,
            document_id TEXT,
            state TEXT NOT NULL DEFAULT 'ready',
            error TEXT,
            created_at INTEGER NOT NULL,
            updated_at INTEGER NOT NULL,
            PRIMARY KEY(object_id, chunk_order, account_id),
            FOREIGN KEY(account_id) REFERENCES telegram_accounts(id) ON DELETE CASCADE
        );
        CREATE INDEX IF NOT EXISTS idx_replica_locations_object
            ON replica_locations(object_id, chunk_order, state);
        CREATE INDEX IF NOT EXISTS idx_replica_locations_account
            ON replica_locations(account_id, state);

        CREATE TABLE IF NOT EXISTS replication_jobs (
            id TEXT PRIMARY KEY,
            source_account_id TEXT NOT NULL,
            target_account_id TEXT NOT NULL,
            bucket TEXT NOT NULL,
            object_keys_json TEXT NOT NULL DEFAULT '[]',
            mode TEXT NOT NULL,
            access_mode TEXT NOT NULL,
            state TEXT NOT NULL DEFAULT 'queued',
            objects_total INTEGER NOT NULL DEFAULT 0,
            objects_done INTEGER NOT NULL DEFAULT 0,
            chunks_total INTEGER NOT NULL DEFAULT 0,
            chunks_done INTEGER NOT NULL DEFAULT 0,
            bytes_done INTEGER NOT NULL DEFAULT 0,
            next_run INTEGER,
            last_run INTEGER,
            error TEXT,
            created_at INTEGER NOT NULL,
            updated_at INTEGER NOT NULL,
            FOREIGN KEY(source_account_id) REFERENCES telegram_accounts(id),
            FOREIGN KEY(target_account_id) REFERENCES telegram_accounts(id)
        );
        CREATE INDEX IF NOT EXISTS idx_replication_jobs_due
            ON replication_jobs(state, next_run, updated_at);

        CREATE TABLE IF NOT EXISTS rechunk_jobs (
            id TEXT PRIMARY KEY,
            bucket TEXT NOT NULL,
            object_key TEXT NOT NULL,
            object_id TEXT NOT NULL,
            source_account_id TEXT NOT NULL DEFAULT 'legacy',
            new_chunk_size INTEGER NOT NULL,
            apply_to_replicas INTEGER NOT NULL DEFAULT 0,
            replica_targets_json TEXT NOT NULL DEFAULT '[]',
            state TEXT NOT NULL DEFAULT 'queued',
            chunks_total INTEGER NOT NULL DEFAULT 0,
            chunks_done INTEGER NOT NULL DEFAULT 0,
            bytes_done INTEGER NOT NULL DEFAULT 0,
            error TEXT,
            created_at INTEGER NOT NULL,
            updated_at INTEGER NOT NULL
        );
        CREATE INDEX IF NOT EXISTS idx_rechunk_jobs_state
            ON rechunk_jobs(state, updated_at);

        CREATE TABLE IF NOT EXISTS rechunk_locks (
            object_id TEXT PRIMARY KEY,
            job_id TEXT NOT NULL UNIQUE,
            reason TEXT NOT NULL,
            created_at INTEGER NOT NULL
        );
        "#,
    )?;

    let bootstrap: Option<String> = connection
        .query_row(
            "SELECT value FROM app_settings WHERE key='telegram_bootstrap'",
            [],
            |row| row.get(0),
        )
        .optional()?;
    let connection_id: Option<String> = connection
        .query_row(
            "SELECT value FROM app_settings WHERE key='telegram_active_connection_id'",
            [],
            |row| row.get(0),
        )
        .optional()?;
    if let (Some(bootstrap), Some(connection_id)) = (bootstrap, connection_id) {
        let now = crate::durable::now();
        connection.execute(
            "INSERT OR IGNORE INTO telegram_accounts(id,label,bootstrap_json,phone,state,created_at,updated_at) VALUES(?1,'Primary account',?2,NULL,'configured',?3,?3)",
            params![connection_id, bootstrap, now],
        )?;
        connection.execute(
            "UPDATE telegram_accounts SET bootstrap_json=?2,updated_at=?3 WHERE id=?1",
            params![connection_id, bootstrap, now],
        )?;
    }
    Ok(())
}

/// Account download eligibility is deliberately separate from ownership and
/// cleanup state. Disabling it only removes the account from read selection;
/// uploads and remote cleanup continue to use their durable owner.
fn ensure_account_download_policy(connection: &mut Connection) -> Result<(), MetadataError> {
    let has_column: bool = connection.query_row(
        "SELECT EXISTS(SELECT 1 FROM pragma_table_info('telegram_accounts') WHERE name='download_enabled')",
        [],
        |row| row.get(0),
    )?;
    if !has_column {
        connection.execute(
            "ALTER TABLE telegram_accounts ADD COLUMN download_enabled INTEGER NOT NULL DEFAULT 1",
            [],
        )?;
    }
    Ok(())
}

/// Account quotas are nullable so an account can explicitly remain unlimited.
/// Usage is derived from committed active manifests and ready physical replica
/// locations; no mutable counter can drift away from the durable object index.
fn ensure_account_quota_schema(connection: &mut Connection) -> Result<(), MetadataError> {
    let has_column: bool = connection.query_row(
        "SELECT EXISTS(SELECT 1 FROM pragma_table_info('telegram_accounts') WHERE name='quota_bytes')",
        [],
        |row| row.get(0),
    )?;
    if !has_column {
        connection.execute(
            "ALTER TABLE telegram_accounts ADD COLUMN quota_bytes INTEGER",
            [],
        )?;
    }
    Ok(())
}

/// Replication scope is additive. An empty JSON array retains the original
/// whole-bucket behavior; populated arrays limit a job to selected object keys.
fn ensure_replication_scope_schema(connection: &mut Connection) -> Result<(), MetadataError> {
    let has_column: bool = connection.query_row(
        "SELECT EXISTS(SELECT 1 FROM pragma_table_info('replication_jobs') WHERE name='object_keys_json')",
        [],
        |row| row.get(0),
    )?;
    if !has_column {
        connection.execute(
            "ALTER TABLE replication_jobs ADD COLUMN object_keys_json TEXT NOT NULL DEFAULT '[]'",
            [],
        )?;
    }
    Ok(())
}

/// Re-chunk replica policy is additive. Existing jobs remain primary-only;
/// new jobs snapshot their replica targets before replacing the old manifest.
fn ensure_rechunk_replica_schema(connection: &mut Connection) -> Result<(), MetadataError> {
    if !table_has_column(connection, "rechunk_jobs", "source_account_id")? {
        connection.execute(
            "ALTER TABLE rechunk_jobs ADD COLUMN source_account_id TEXT NOT NULL DEFAULT 'legacy'",
            [],
        )?;
    }
    if !table_has_column(connection, "rechunk_jobs", "apply_to_replicas")? {
        connection.execute(
            "ALTER TABLE rechunk_jobs ADD COLUMN apply_to_replicas INTEGER NOT NULL DEFAULT 0",
            [],
        )?;
    }
    if !table_has_column(connection, "rechunk_jobs", "replica_targets_json")? {
        connection.execute(
            "ALTER TABLE rechunk_jobs ADD COLUMN replica_targets_json TEXT NOT NULL DEFAULT '[]'",
            [],
        )?;
    }
    Ok(())
}

fn ensure_traffic_totals_schema(connection: &mut Connection) -> Result<(), MetadataError> {
    connection.execute_batch(
        r#"
        CREATE TABLE IF NOT EXISTS traffic_totals (
            id INTEGER PRIMARY KEY CHECK (id = 1),
            client_upload_bytes TEXT NOT NULL DEFAULT '0',
            client_download_bytes TEXT NOT NULL DEFAULT '0',
            telegram_upload_bytes TEXT NOT NULL DEFAULT '0',
            telegram_download_bytes TEXT NOT NULL DEFAULT '0',
            updated_at TEXT NOT NULL
        );
        INSERT OR IGNORE INTO traffic_totals (
            id,
            client_upload_bytes,
            client_download_bytes,
            telegram_upload_bytes,
            telegram_download_bytes,
            updated_at
        ) VALUES (1, '0', '0', '0', '0', '');
        "#,
    )?;
    Ok(())
}

fn ensure_download_prefetch_setting(connection: &mut Connection) -> Result<(), MetadataError> {
    connection.execute(
        "INSERT OR IGNORE INTO app_settings(key, value, updated_at) VALUES (?1, ?2, ?3)",
        params![
            super::settings::DOWNLOAD_PREFETCH_CHUNKS_SETTING,
            DEFAULT_DOWNLOAD_PREFETCH_CHUNKS.to_string(),
            timestamp_now()?
        ],
    )?;
    Ok(())
}

fn ensure_download_prefetch_mode_setting(connection: &mut Connection) -> Result<(), MetadataError> {
    connection.execute(
        "INSERT OR IGNORE INTO app_settings(key, value, updated_at) VALUES (?1, ?2, ?3)",
        params![
            super::settings::DOWNLOAD_PREFETCH_MODE_SETTING,
            DEFAULT_DOWNLOAD_PREFETCH_MODE,
            timestamp_now()?
        ],
    )?;
    Ok(())
}

fn ensure_download_account_connections_setting(
    connection: &mut Connection,
) -> Result<(), MetadataError> {
    connection.execute(
        "INSERT OR IGNORE INTO app_settings(key, value, updated_at) VALUES (?1, ?2, ?3)",
        params![
            super::settings::DOWNLOAD_ACCOUNT_CONNECTIONS_SETTING,
            DEFAULT_DOWNLOAD_ACCOUNT_CONNECTIONS.to_string(),
            timestamp_now()?
        ],
    )?;
    Ok(())
}

fn ensure_download_failover_setting(connection: &mut Connection) -> Result<(), MetadataError> {
    connection.execute(
        "INSERT OR IGNORE INTO app_settings(key, value, updated_at) VALUES (?1, ?2, ?3)",
        params![
            super::settings::DOWNLOAD_FAILOVER_RETRIES_SETTING,
            DEFAULT_DOWNLOAD_FAILOVER_RETRIES.to_string(),
            timestamp_now()?
        ],
    )?;
    Ok(())
}

fn ensure_cleanup_retention_setting(connection: &mut Connection) -> Result<(), MetadataError> {
    connection.execute(
        "INSERT OR IGNORE INTO app_settings(key, value, updated_at) VALUES (?1, ?2, ?3)",
        params![
            super::settings::CLEANUP_RETENTION_SETTING,
            DEFAULT_CLEANUP_RETENTION_SECS.to_string(),
            timestamp_now()?
        ],
    )?;
    Ok(())
}

fn ensure_recovery_verify_startup_setting(
    connection: &mut Connection,
) -> Result<(), MetadataError> {
    connection.execute(
        "INSERT OR IGNORE INTO app_settings(key, value, updated_at) VALUES (?1, ?2, ?3)",
        params![
            "telegram_recovery_verify_startup",
            DEFAULT_RECOVERY_VERIFY_STARTUP.to_string(),
            timestamp_now()?
        ],
    )?;
    Ok(())
}

fn ensure_share_schema(connection: &mut Connection) -> Result<(), MetadataError> {
    connection.execute_batch(
        r#"
        CREATE TABLE IF NOT EXISTS share_links (
            id TEXT PRIMARY KEY,
            token_hash TEXT NOT NULL UNIQUE,
            token_ciphertext TEXT,
            object_id TEXT NOT NULL,
            bucket TEXT NOT NULL,
            object_key TEXT NOT NULL,
            created_at INTEGER NOT NULL,
            expires_at INTEGER,
            revoked_at INTEGER,
            description TEXT NOT NULL DEFAULT ''
        );
        CREATE INDEX IF NOT EXISTS idx_share_links_object
            ON share_links(object_id);
        CREATE INDEX IF NOT EXISTS idx_share_links_expiry
            ON share_links(expires_at);
        "#,
    )?;
    if !table_has_column(connection, "share_links", "token_ciphertext")? {
        connection.execute(
            "ALTER TABLE share_links ADD COLUMN token_ciphertext TEXT",
            [],
        )?;
    }
    if !table_has_column(connection, "share_links", "description")? {
        connection.execute(
            "ALTER TABLE share_links ADD COLUMN description TEXT NOT NULL DEFAULT ''",
            [],
        )?;
    }
    Ok(())
}

fn ensure_connection_removal_schema(connection: &mut Connection) -> Result<(), MetadataError> {
    connection.execute_batch(
        r#"
        CREATE TABLE IF NOT EXISTS connection_removal_jobs (
            id TEXT PRIMARY KEY,
            connection_id TEXT NOT NULL DEFAULT 'legacy',
            delete_uploaded_files INTEGER NOT NULL,
            state TEXT NOT NULL,
            object_count INTEGER NOT NULL DEFAULT 0,
            requested_at INTEGER NOT NULL,
            updated_at INTEGER NOT NULL,
            completed_at INTEGER,
            error TEXT
        );
        CREATE INDEX IF NOT EXISTS idx_connection_removal_jobs_state
            ON connection_removal_jobs(state, requested_at);
        CREATE TABLE IF NOT EXISTS connection_removal_objects (
            job_id TEXT NOT NULL REFERENCES connection_removal_jobs(id) ON DELETE CASCADE,
            object_id TEXT NOT NULL,
            PRIMARY KEY(job_id, object_id)
        );
        CREATE INDEX IF NOT EXISTS idx_connection_removal_objects_object
            ON connection_removal_objects(object_id);
        "#,
    )?;
    if !column_exists(connection, "connection_removal_jobs", "connection_id")? {
        connection.execute(
            "ALTER TABLE connection_removal_jobs ADD COLUMN connection_id TEXT NOT NULL DEFAULT 'legacy'",
            [],
        )?;
    }
    Ok(())
}

fn ensure_phase10_schema(connection: &mut Connection) -> Result<(), MetadataError> {
    if !table_exists(connection, "cleanup_targets")? {
        return Ok(());
    }
    for (column, definition) in [
        ("connection_id", "TEXT NOT NULL DEFAULT 'legacy'"),
        ("target_kind", "TEXT NOT NULL DEFAULT 'message'"),
        ("state", "TEXT NOT NULL DEFAULT 'pending'"),
        ("attempts", "INTEGER NOT NULL DEFAULT 0"),
        ("next_retry", "INTEGER NOT NULL DEFAULT 0"),
        ("lease", "TEXT"),
        ("lease_until", "INTEGER NOT NULL DEFAULT 0"),
        ("write_conditionals_json", "TEXT"),
        ("error", "TEXT"),
        ("evidence_location_json", "TEXT"),
        ("evidence_attempt_token", "TEXT"),
        ("evidence_attempt_started_at", "INTEGER"),
    ] {
        if !column_exists(connection, "cleanup_targets", column)? {
            connection.execute(
                &format!("ALTER TABLE cleanup_targets ADD COLUMN {column} {definition}"),
                [],
            )?;
        }
    }
    for table in [
        "buckets",
        "object_manifests",
        "multipart_uploads",
        "transfer_jobs",
    ] {
        if !column_exists(connection, table, "connection_id")? {
            connection.execute(
                &format!(
                    "ALTER TABLE {table} ADD COLUMN connection_id TEXT NOT NULL DEFAULT 'legacy'"
                ),
                [],
            )?;
            connection.execute(
                &format!("UPDATE {table} SET connection_id=COALESCE((SELECT value FROM app_settings WHERE key='telegram_active_connection_id'),'legacy') WHERE connection_id='legacy'"),
                [],
            )?;
        }
        connection.execute(
            &format!("CREATE INDEX IF NOT EXISTS idx_{table}_connection ON {table}(connection_id)"),
            [],
        )?;
    }
    connection.execute_batch(
        r#"
        CREATE INDEX IF NOT EXISTS idx_cleanup_due
            ON cleanup_targets(state, due_at, next_retry, id);
        CREATE INDEX IF NOT EXISTS idx_cleanup_claim
            ON cleanup_targets(target_kind, completed, state, due_at, next_retry, id);
        CREATE INDEX IF NOT EXISTS idx_cleanup_object_kind_completed
            ON cleanup_targets(object_id, target_kind, completed);
        CREATE TABLE IF NOT EXISTS transfer_send_attempts (
            job_id TEXT NOT NULL REFERENCES transfer_jobs(id),
            chunk_order INTEGER NOT NULL,
            attempt_id TEXT NOT NULL,
            attempt_token TEXT NOT NULL DEFAULT '',
            state TEXT NOT NULL,
            started_at INTEGER NOT NULL,
            finished_at INTEGER,
            location_json TEXT,
            error_kind TEXT,
            error TEXT,
            retryable INTEGER NOT NULL DEFAULT 0,
            PRIMARY KEY(job_id, chunk_order, attempt_id)
        );
        CREATE INDEX IF NOT EXISTS idx_transfer_send_attempt_state
            ON transfer_send_attempts(job_id, state);
        "#,
    )?;
    for (column, definition) in [
        ("attempt_token", "TEXT NOT NULL DEFAULT ''"),
        ("error_kind", "TEXT"),
        ("error", "TEXT"),
        ("retryable", "INTEGER NOT NULL DEFAULT 0"),
    ] {
        if !column_exists(connection, "transfer_send_attempts", column)? {
            connection.execute(
                &format!("ALTER TABLE transfer_send_attempts ADD COLUMN {column} {definition}"),
                [],
            )?;
        }
    }
    Ok(())
}

fn backfill_cleanup_outbox(connection: &mut Connection) -> Result<(), MetadataError> {
    if !table_exists(connection, "cleanup_targets")? {
        return Ok(());
    }
    let retention_secs = cleanup_retention_seconds(connection)?;
    let manifests = {
        let mut statement = connection.prepare(
            "SELECT manifest_json, tombstoned_at FROM object_manifests WHERE commit_state='tombstoned'",
        )?;
        statement
            .query_map([], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, Option<String>>(1)?))
            })?
            .collect::<Result<Vec<_>, _>>()?
    };
    let tx = connection.transaction()?;
    for (json, tombstoned_at) in manifests {
        let manifest: crate::manifest::ObjectManifest = serde_json::from_str(&json)?;
        let due_at = tombstoned_at
            .as_deref()
            .map(parse_rfc3339_timestamp)
            .transpose()?
            .map(|value| (value + time::Duration::seconds(retention_secs)).unix_timestamp())
            .unwrap_or_else(|| crate::durable::now() + retention_secs);
        crate::durable::enqueue_manifest_cleanup_at(&tx, &manifest, due_at)?;
    }
    tx.commit()?;
    Ok(())
}

fn cleanup_retention_seconds(connection: &Connection) -> Result<i64, MetadataError> {
    let has_settings: bool = connection.query_row(
        "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name='app_settings')",
        [],
        |row| row.get(0),
    )?;
    if !has_settings {
        return Ok(DEFAULT_CLEANUP_RETENTION_SECS as i64);
    }
    let value: Option<String> = connection
        .query_row(
            "SELECT value FROM app_settings WHERE key=?1",
            [super::settings::CLEANUP_RETENTION_SETTING],
            |row| row.get(0),
        )
        .optional()?;
    Ok(value
        .map(|value| {
            value.parse::<i64>().map_err(|_| {
                MetadataError::InvalidManifest("invalid stored cleanup retention".into())
            })
        })
        .transpose()?
        .unwrap_or(DEFAULT_CLEANUP_RETENTION_SECS as i64))
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

fn table_has_column(
    connection: &Connection,
    table: &str,
    column: &str,
) -> Result<bool, MetadataError> {
    let mut statement = connection.prepare(&format!("PRAGMA table_info({table})"))?;
    let mut rows = statement.query([])?;
    while let Some(row) = rows.next()? {
        if row.get::<_, String>(1)? == column {
            return Ok(true);
        }
    }
    Ok(false)
}

fn column_exists(
    connection: &Connection,
    table: &str,
    column: &str,
) -> Result<bool, MetadataError> {
    let mut statement = connection.prepare(&format!("PRAGMA table_info({table})"))?;
    let mut rows = statement.query([])?;
    while let Some(row) = rows.next()? {
        if row.get::<_, String>(1)? == column {
            return Ok(true);
        }
    }
    Ok(false)
}

fn create_private_metadata_file(path: &std::path::Path) -> Result<(), MetadataError> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::metadata::TrafficCounterKind;
    use tempfile::tempdir;

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
    fn migration_from_version_twelve_adds_download_prefetch_policy() {
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
                    VALUES (1, 12, '2026-09-26T00:00:00Z');
                    "#,
                )
                .expect("seed");
        }

        let store = MetadataStore::open(&path).expect("migrate");
        assert_eq!(store.schema_version().expect("schema"), SCHEMA_VERSION);
        assert_eq!(
            store
                .telegram_download_prefetch_chunks()
                .expect("prefetch setting"),
            Some(DEFAULT_DOWNLOAD_PREFETCH_CHUNKS)
        );
    }

    #[test]
    fn migration_from_version_thirteen_adds_persistent_traffic_totals() {
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
                    VALUES (1, 13, '2026-09-27T00:00:00Z');
                    "#,
                )
                .expect("seed");
        }

        let store = MetadataStore::open(&path).expect("migrate");
        assert_eq!(store.schema_version().expect("schema"), SCHEMA_VERSION);
        store
            .increment_traffic_total(TrafficCounterKind::TelegramDownload, 42)
            .expect("increment");
        assert_eq!(
            store
                .traffic_totals()
                .expect("totals")
                .telegram_download_bytes,
            42
        );
        drop(store);

        let reopened = MetadataStore::open(&path).expect("reopen");
        assert_eq!(
            reopened
                .traffic_totals()
                .expect("reopened totals")
                .telegram_download_bytes,
            42
        );
    }

    #[test]
    fn schema_v16_adds_account_and_maintenance_tables_and_scope_column() {
        let store = MetadataStore::open_in_memory().expect("open");
        store
            .with_connection(|connection| {
                for table in [
                    "telegram_accounts",
                    "replica_locations",
                    "replication_jobs",
                    "rechunk_jobs",
                    "rechunk_locks",
                    "integrity_recovery_events",
                ] {
                    let found: Option<String> = connection
                        .query_row(
                            "SELECT name FROM sqlite_master WHERE type='table' AND name=?1",
                            [table],
                            |row| row.get(0),
                        )
                        .optional()?;
                    assert_eq!(found.as_deref(), Some(table));
                }
                let has_scope: bool = connection.query_row(
                    "SELECT EXISTS(SELECT 1 FROM pragma_table_info('replication_jobs') WHERE name='object_keys_json')",
                    [],
                    |row| row.get(0),
                )?;
                assert!(has_scope);
                Ok(())
            })
            .expect("tables");
    }

    #[test]
    fn migration_from_version_fifteen_adds_replication_scope_without_losing_jobs() {
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
                    VALUES (1, 15, '2026-09-27T00:00:00Z');
                    CREATE TABLE replication_jobs (
                        id TEXT PRIMARY KEY,
                        source_account_id TEXT NOT NULL,
                        target_account_id TEXT NOT NULL,
                        bucket TEXT NOT NULL,
                        mode TEXT NOT NULL,
                        access_mode TEXT NOT NULL,
                        state TEXT NOT NULL DEFAULT 'queued',
                        objects_total INTEGER NOT NULL DEFAULT 0,
                        objects_done INTEGER NOT NULL DEFAULT 0,
                        chunks_total INTEGER NOT NULL DEFAULT 0,
                        chunks_done INTEGER NOT NULL DEFAULT 0,
                        bytes_done INTEGER NOT NULL DEFAULT 0,
                        next_run INTEGER,
                        last_run INTEGER,
                        error TEXT,
                        created_at INTEGER NOT NULL,
                        updated_at INTEGER NOT NULL
                    );
                    INSERT INTO replication_jobs(id,source_account_id,target_account_id,bucket,mode,access_mode,created_at,updated_at)
                    VALUES ('legacy-job','source','target','backups','one_time','replica',1,1);
                    "#,
                )
                .expect("seed");
        }

        let store = MetadataStore::open(&path).expect("migrate");
        assert_eq!(store.schema_version().expect("version"), SCHEMA_VERSION);
        let jobs = store.list_replication_jobs().expect("jobs");
        assert_eq!(jobs.len(), 1);
        assert!(jobs[0].object_keys.is_empty());
    }

    #[test]
    fn migration_from_version_sixteen_adds_download_policy_without_changing_accounts() {
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
                    VALUES (1, 16, '2026-09-27T00:00:00Z');
                    CREATE TABLE app_settings (key TEXT PRIMARY KEY, value TEXT NOT NULL, updated_at TEXT NOT NULL);
                    CREATE TABLE telegram_accounts (
                        id TEXT PRIMARY KEY,
                        label TEXT NOT NULL,
                        bootstrap_json TEXT NOT NULL,
                        phone TEXT,
                        state TEXT NOT NULL DEFAULT 'configured',
                        created_at INTEGER NOT NULL,
                        updated_at INTEGER NOT NULL
                    );
                    INSERT INTO telegram_accounts(id,label,bootstrap_json,state,created_at,updated_at)
                    VALUES ('old-account','Old account','{}','configured',1,1);
                    "#,
                )
                .expect("seed");
        }

        let store = MetadataStore::open(&path).expect("migrate");
        assert_eq!(store.schema_version().expect("version"), SCHEMA_VERSION);
        let account = store
            .list_telegram_accounts()
            .expect("accounts")
            .into_iter()
            .next()
            .expect("old account");
        assert!(account.download_enabled);
        store
            .with_connection(|connection| {
                connection.execute(
                    "UPDATE telegram_accounts SET download_enabled=0 WHERE id='old-account'",
                    [],
                )?;
                Ok(())
            })
            .expect("disable policy");
        assert!(
            !store
                .telegram_account_download_enabled("old-account")
                .expect("policy")
        );
    }

    #[test]
    fn migration_from_version_seventeen_adds_rechunk_replica_policy() {
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
                    VALUES (1, 17, '2026-09-28T00:00:00Z');
                    CREATE TABLE rechunk_jobs (
                        id TEXT PRIMARY KEY,
                        bucket TEXT NOT NULL,
                        object_key TEXT NOT NULL,
                        object_id TEXT NOT NULL,
                        new_chunk_size INTEGER NOT NULL,
                        state TEXT NOT NULL DEFAULT 'queued',
                        chunks_total INTEGER NOT NULL DEFAULT 0,
                        chunks_done INTEGER NOT NULL DEFAULT 0,
                        bytes_done INTEGER NOT NULL DEFAULT 0,
                        error TEXT,
                        created_at INTEGER NOT NULL,
                        updated_at INTEGER NOT NULL
                    );
                    "#,
                )
                .expect("seed");
        }

        let store = MetadataStore::open(&path).expect("migrate");
        assert_eq!(store.schema_version().expect("version"), SCHEMA_VERSION);
        store
            .with_connection(|connection| {
                for column in ["source_account_id", "apply_to_replicas", "replica_targets_json"] {
                    let found: bool = connection.query_row(
                        "SELECT EXISTS(SELECT 1 FROM pragma_table_info('rechunk_jobs') WHERE name=?1)",
                        [column],
                        |row| row.get(0),
                    )?;
                    assert!(found, "missing migrated column {column}");
                }
                Ok(())
            })
            .expect("columns");
    }

    #[test]
    fn migration_from_version_eighteen_adds_download_failover_setting() {
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
                    VALUES (1, 18, '2026-09-28T00:00:00Z');
                    CREATE TABLE app_settings (
                        key TEXT PRIMARY KEY,
                        value TEXT NOT NULL,
                        updated_at TEXT NOT NULL
                    );
                    INSERT INTO app_settings(key, value, updated_at)
                    VALUES ('telegram_download_prefetch_chunks', '2', '2026-09-28T00:00:00Z');
                    "#,
                )
                .expect("seed");
        }

        let store = MetadataStore::open(&path).expect("migrate");
        assert_eq!(store.schema_version().expect("version"), SCHEMA_VERSION);
        assert_eq!(
            store
                .telegram_download_failover_retries()
                .expect("failover setting"),
            Some(1)
        );
        assert_eq!(
            store
                .telegram_download_prefetch_chunks()
                .expect("prefetch setting"),
            Some(2)
        );
    }

    #[test]
    fn migration_from_version_nineteen_adds_integrity_recovery_events() {
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
                    VALUES (1, 19, '2026-09-28T00:00:00Z');
                    "#,
                )
                .expect("seed");
        }

        let store = MetadataStore::open(&path).expect("migrate");
        assert_eq!(store.schema_version().expect("version"), SCHEMA_VERSION);
        store
            .with_connection(|connection| {
                let found: bool = connection.query_row(
                    "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name='integrity_recovery_events')",
                    [],
                    |row| row.get(0),
                )?;
                assert!(found);
                Ok(())
            })
            .expect("event table");
        assert_eq!(
            store
                .telegram_recovery_verify_startup()
                .expect("startup setting"),
            Some(true)
        );
    }

    #[test]
    fn migration_from_version_twenty_one_adds_startup_verifier_setting() {
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
                    VALUES (1, 21, '2026-09-29T00:00:00Z');
                    "#,
                )
                .expect("seed");
        }

        let store = MetadataStore::open(&path).expect("migrate");
        assert_eq!(store.schema_version().expect("version"), SCHEMA_VERSION);
        assert_eq!(
            store
                .telegram_recovery_verify_startup()
                .expect("startup setting"),
            Some(true)
        );
    }

    #[test]
    fn migration_from_version_twenty_two_adds_download_prefetch_mode() {
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
                    VALUES (1, 22, '2026-09-30T00:00:00Z');
                    "#,
                )
                .expect("seed");
        }

        let store = MetadataStore::open(&path).expect("migrate");
        assert_eq!(store.schema_version().expect("version"), SCHEMA_VERSION);
        assert_eq!(
            store
                .telegram_download_prefetch_mode()
                .expect("prefetch mode"),
            Some(DEFAULT_DOWNLOAD_PREFETCH_MODE.to_string())
        );
    }

    #[test]
    fn migration_from_version_twenty_three_adds_account_connection_limit() {
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
                    VALUES (1, 23, '2026-10-02T00:00:00Z');
                    "#,
                )
                .expect("seed");
        }

        let store = MetadataStore::open(&path).expect("migrate");
        assert_eq!(store.schema_version().expect("version"), SCHEMA_VERSION);
        assert_eq!(
            store
                .telegram_download_account_connections()
                .expect("account connection limit"),
            Some(DEFAULT_DOWNLOAD_ACCOUNT_CONNECTIONS)
        );
    }

    #[test]
    fn auth_tables_created_and_migrate_is_idempotent() {
        let store = MetadataStore::open_in_memory().expect("open");
        assert_eq!(store.schema_version().expect("version"), SCHEMA_VERSION);
        store.migrate().expect("migrate");
        assert_eq!(store.schema_version().expect("version"), SCHEMA_VERSION);
        store
            .create_user("uA", "dave", "hash", "admin", "")
            .expect("create after migrate");
    }

    #[test]
    fn backfill_cleanup_outbox_preserves_tombstoned_due_time() {
        let mut connection = Connection::open_in_memory().expect("open");
        connection
            .execute_batch(
                r#"
                CREATE TABLE object_manifests (
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
                CREATE TABLE cleanup_targets (
                    id INTEGER PRIMARY KEY AUTOINCREMENT,
                    object_id TEXT NOT NULL,
                    peer_id TEXT NOT NULL,
                    message_id INTEGER NOT NULL,
                    connection_id TEXT NOT NULL DEFAULT 'legacy',
                    target_kind TEXT NOT NULL DEFAULT 'message',
                    due_at INTEGER NOT NULL,
                    state TEXT NOT NULL DEFAULT 'pending',
                    attempts INTEGER NOT NULL DEFAULT 0,
                    next_retry INTEGER NOT NULL DEFAULT 0,
                    lease TEXT,
                    lease_until INTEGER NOT NULL DEFAULT 0,
                    error TEXT,
                    evidence_location_json TEXT,
                    completed INTEGER NOT NULL DEFAULT 0,
                    UNIQUE(peer_id, message_id)
                );
                "#,
            )
            .expect("schema");
        let mut manifest =
            crate::manifest::ObjectManifest::committed(crate::manifest::CommittedManifestArgs {
                bucket: "bucket".to_string(),
                key: "key.txt".to_string(),
                content_length: 3,
                content_type: "text/plain".to_string(),
                checksum_algorithm: "sha256".to_string(),
                whole_object: "abcd".to_string(),
                peer_id: "peer".to_string(),
                message_id: 42,
            });
        manifest.commit_state = crate::manifest::CommitState::Tombstoned;
        let tombstoned_at = "2026-09-01T00:00:00Z";
        let expected_due_at = parse_rfc3339_timestamp(tombstoned_at)
            .expect("parse")
            .unix_timestamp()
            + crate::object_format::GARBAGE_COLLECTION_RETENTION_SECONDS;
        let manifest_json = serde_json::to_string(&manifest).expect("serialize");
        connection
            .execute(
                "INSERT INTO object_manifests (object_id,bucket,object_key,version_id,commit_state,manifest_json,created_at,committed_at,tombstoned_at) VALUES (?1,?2,?3,?4,'tombstoned',?5,?6,NULL,?7)",
                params![
                    manifest.object_id.to_string(),
                    manifest.bucket,
                    manifest.key,
                    manifest.version_id,
                    manifest_json,
                    "2026-09-01T00:00:00Z",
                    tombstoned_at,
                ],
            )
            .expect("insert manifest");

        backfill_cleanup_outbox(&mut connection).expect("backfill");

        let due_at: i64 = connection
            .query_row(
                "SELECT due_at FROM cleanup_targets WHERE target_kind='evidence' AND peer_id=?1",
                [format!("evidence:{}", manifest.object_id)],
                |row| row.get(0),
            )
            .expect("due at");
        assert_eq!(due_at, expected_due_at);
    }
}
