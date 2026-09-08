use super::rows::{count_rows, parse_rfc3339_timestamp, timestamp_now};
use super::{MetadataError, MetadataStatus, MetadataStore, SCHEMA_VERSION};
use rusqlite::{Connection, OptionalExtension, params};

impl MetadataStore {
    pub fn open(path: impl AsRef<std::path::Path>) -> Result<Self, MetadataError> {
        let path = path.as_ref().to_path_buf();
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        create_private_metadata_file(&path)?;

        let connection = Connection::open(&path)?;
        connection.busy_timeout(std::time::Duration::from_secs(30))?;
        connection.pragma_update(None, "foreign_keys", true)?;
        connection.pragma_update(None, "journal_mode", "WAL")?;
        connection.pragma_update(None, "synchronous", "FULL")?;

        let store = Self {
            path: Some(path),
            connection: std::sync::Mutex::new(connection),
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
        let connection = Connection::open_in_memory()?;
        connection.busy_timeout(std::time::Duration::from_secs(30))?;
        connection.pragma_update(None, "foreign_keys", true)?;
        connection.pragma_update(None, "synchronous", "FULL")?;

        let store = Self {
            path: None,
            connection: std::sync::Mutex::new(connection),
        };
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

        CREATE TABLE IF NOT EXISTS app_settings (
            key TEXT PRIMARY KEY,
            value TEXT NOT NULL,
            updated_at TEXT NOT NULL
        );

        CREATE TABLE IF NOT EXISTS transfer_jobs (
            sequence INTEGER PRIMARY KEY AUTOINCREMENT,
            id TEXT NOT NULL UNIQUE,
            object_id TEXT NOT NULL,
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
            state TEXT NOT NULL,
            started_at INTEGER NOT NULL,
            finished_at INTEGER,
            location_json TEXT,
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
        CREATE INDEX IF NOT EXISTS idx_cleanup_due
            ON cleanup_targets(state, due_at, next_retry, id);
        INSERT OR IGNORE INTO app_settings(key, value, updated_at)
            SELECT 'setup_complete', CASE WHEN EXISTS(SELECT 1 FROM users) THEN 'true' ELSE 'false' END, '';
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
    ensure_phase10_schema(connection)?;
    Ok(())
}

fn ensure_phase10_schema(connection: &mut Connection) -> Result<(), MetadataError> {
    if !table_exists(connection, "cleanup_targets")? {
        return Ok(());
    }
    for (column, definition) in [
        ("target_kind", "TEXT NOT NULL DEFAULT 'message'"),
        ("state", "TEXT NOT NULL DEFAULT 'pending'"),
        ("attempts", "INTEGER NOT NULL DEFAULT 0"),
        ("next_retry", "INTEGER NOT NULL DEFAULT 0"),
        ("lease", "TEXT"),
        ("lease_until", "INTEGER NOT NULL DEFAULT 0"),
        ("write_conditionals_json", "TEXT"),
        ("error", "TEXT"),
        ("evidence_location_json", "TEXT"),
    ] {
        if !column_exists(connection, "cleanup_targets", column)? {
            connection.execute(
                &format!("ALTER TABLE cleanup_targets ADD COLUMN {column} {definition}"),
                [],
            )?;
        }
    }
    connection.execute_batch(
        r#"
        CREATE INDEX IF NOT EXISTS idx_cleanup_due
            ON cleanup_targets(state, due_at, next_retry, id);
        CREATE TABLE IF NOT EXISTS transfer_send_attempts (
            job_id TEXT NOT NULL REFERENCES transfer_jobs(id),
            chunk_order INTEGER NOT NULL,
            attempt_id TEXT NOT NULL,
            state TEXT NOT NULL,
            started_at INTEGER NOT NULL,
            finished_at INTEGER,
            location_json TEXT,
            PRIMARY KEY(job_id, chunk_order, attempt_id)
        );
        CREATE INDEX IF NOT EXISTS idx_transfer_send_attempt_state
            ON transfer_send_attempts(job_id, state);
        "#,
    )?;
    Ok(())
}

fn backfill_cleanup_outbox(connection: &mut Connection) -> Result<(), MetadataError> {
    if !table_exists(connection, "cleanup_targets")? {
        return Ok(());
    }
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
            .map(|value| {
                (value
                    + time::Duration::seconds(
                        crate::object_format::GARBAGE_COLLECTION_RETENTION_SECONDS,
                    ))
                .unix_timestamp()
            })
            .unwrap_or_else(|| crate::durable::now() + 7 * 86400);
        crate::durable::enqueue_manifest_cleanup_at(&tx, &manifest, due_at)?;
    }
    tx.commit()?;
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

#[cfg(test)]
mod tests {
    use super::*;
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
