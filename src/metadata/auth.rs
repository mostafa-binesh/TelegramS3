use super::rows::{parse_timestamp, row_user, timestamp_now};
use super::{MetadataError, MetadataStore};
use crate::manifest::CommitState;
use rusqlite::{OptionalExtension, params};
use s3s::dto::{ETagCondition, Timestamp};
use time::OffsetDateTime;
use time::format_description::well_known::Rfc3339;

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
        let id = id.to_string();
        self.with_connection(|connection| {
            let tx = connection.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
            let first: bool = tx.query_row("SELECT NOT EXISTS(SELECT 1 FROM users)", [], |r| r.get(0))?;
            let effective_role = if first { "superadmin" } else { role };
            tx.execute(
                r#"
                INSERT INTO users (
                    id, username, password_hash, role, display_name,
                    disabled, token_version, created_at, updated_at
                )
                VALUES (?1, ?2, ?3, ?4, ?5, 0, 0, ?6, ?6)
                "#,
                params![id, username, password_hash, effective_role, display_name, now],
            )?;
            tx.execute(
                "INSERT INTO app_settings(key,value,updated_at) VALUES ('setup_complete','true',?1) ON CONFLICT(key) DO UPDATE SET value='true',updated_at=excluded.updated_at",
                [now],
            )?;
            tx.commit()?;
            Ok(())
        })?;
        self.get_user_by_id(&id)?
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
                out.push(super::rows::read_user_row(row)?);
            }
            Ok(out)
        })
    }

    pub fn user_count(&self) -> Result<u64, MetadataError> {
        self.with_connection(|conn| {
            Ok(conn.query_row("SELECT COUNT(*) FROM users", [], |r| r.get::<_, u64>(0))?)
        })
    }

    pub fn enabled_superadmin_count(&self) -> Result<u64, MetadataError> {
        self.with_connection(|conn| {
            Ok(conn.query_row(
                "SELECT COUNT(*) FROM users WHERE role='superadmin' AND disabled=0",
                [],
                |r| r.get::<_, u64>(0),
            )?)
        })
    }

    pub fn delete_user(&self, id: &str) -> Result<(), MetadataError> {
        self.with_connection(|connection| {
            let tx =
                connection.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
            let role: Option<String> = tx
                .query_row("SELECT role FROM users WHERE id=?1", [id], |r| r.get(0))
                .optional()?;
            if role.as_deref() == Some("superadmin") {
                let superadmins: u64 = tx.query_row(
                    "SELECT COUNT(*) FROM users WHERE role='superadmin' AND disabled=0",
                    [],
                    |r| r.get(0),
                )?;
                if superadmins <= 1 {
                    return Err(MetadataError::InvalidManifest(
                        "cannot delete the last enabled superadmin".into(),
                    ));
                }
            }
            tx.execute("DELETE FROM users WHERE id = ?1", params![id])?;
            tx.commit()?;
            Ok(())
        })
    }

    pub fn delete_active_key(
        &self,
        bucket: &str,
        object_key: &str,
        reason: &str,
        if_match: Option<&ETagCondition>,
        if_match_last_modified_time: Option<&Timestamp>,
        if_match_size: Option<i64>,
    ) -> Result<Option<crate::manifest::ObjectManifest>, MetadataError> {
        self.with_connection(|connection| {
            let tx =
                connection.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
            let deleted = delete_active_key_in_transaction(
                &tx,
                bucket,
                object_key,
                reason,
                if_match,
                if_match_last_modified_time,
                if_match_size,
            )?;
            tx.commit()?;
            Ok(deleted)
        })
    }

    pub fn delete_empty_folder(
        &self,
        bucket: &str,
        folder_key: &str,
        reason: &str,
    ) -> Result<Option<crate::manifest::ObjectManifest>, MetadataError> {
        if !folder_key.ends_with('/') {
            return Err(MetadataError::InvalidManifest(
                "folder key must end with '/'".into(),
            ));
        }
        self.with_connection(|connection| {
            let tx =
                connection.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
            let has_children: bool = tx.query_row(
                "SELECT EXISTS(SELECT 1 FROM active_objects WHERE bucket=?1 AND instr(object_key, ?2)=1 AND object_key<>?2)",
                params![bucket, folder_key],
                |row| row.get(0),
            )?;
            let has_pending_children: bool = tx.query_row(
                "SELECT EXISTS(SELECT 1 FROM transfer_jobs WHERE bucket=?1 AND instr(object_key, ?2)=1 AND object_key<>?2 AND state NOT IN ('completed','cancelled','cleaned'))",
                params![bucket, folder_key],
                |row| row.get(0),
            )?;
            if has_children || has_pending_children {
                return Err(MetadataError::FolderNotEmpty(folder_key.to_string()));
            }
            let deleted = delete_active_key_in_transaction(
                &tx,
                bucket,
                folder_key,
                reason,
                None,
                None,
                None,
            )?;
            tx.commit()?;
            Ok(deleted)
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
            let tx =
                connection.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
            if disabled {
                let role: Option<String> = tx
                    .query_row(
                        "SELECT role FROM users WHERE id=?1 AND disabled=0",
                        [id],
                        |r| r.get(0),
                    )
                    .optional()?;
                if role.as_deref() == Some("superadmin") {
                    let superadmins: u64 = tx.query_row(
                        "SELECT COUNT(*) FROM users WHERE role='superadmin' AND disabled=0",
                        [],
                        |r| r.get(0),
                    )?;
                    if superadmins <= 1 {
                        return Err(MetadataError::InvalidManifest(
                            "cannot disable the last enabled superadmin".into(),
                        ));
                    }
                }
            }
            tx.execute(
                "UPDATE users SET disabled = ?2, updated_at = ?3 WHERE id = ?1",
                params![id, disabled, now],
            )?;
            tx.commit()?;
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

fn delete_active_key_in_transaction(
    tx: &rusqlite::Transaction<'_>,
    bucket: &str,
    object_key: &str,
    reason: &str,
    if_match: Option<&ETagCondition>,
    if_match_last_modified_time: Option<&Timestamp>,
    if_match_size: Option<i64>,
) -> Result<Option<crate::manifest::ObjectManifest>, MetadataError> {
    let active_transfer: bool = tx.query_row(
        "SELECT EXISTS(SELECT 1 FROM transfer_jobs WHERE bucket=?1 AND object_key=?2 AND state IN ('uploading','committing'))",
        params![bucket, object_key],
        |r| r.get(0),
    )?;
    if active_transfer {
        return Err(MetadataError::InvalidManifest(
            "object publication in progress; retry delete".into(),
        ));
    }
    tx.execute(
        "UPDATE transfer_jobs SET state='cancelled',lease=NULL,lease_until=0,error='Superseded by deletion',updated_at=?3 WHERE bucket=?1 AND object_key=?2 AND state IN ('receiving','queued','retry_wait','recovery_required')",
        params![bucket, object_key, crate::durable::now()],
    )?;

    let object_id: Option<String> = tx
        .query_row(
            "SELECT object_id FROM active_objects WHERE bucket=?1 AND object_key=?2",
            params![bucket, object_key],
            |r| r.get(0),
        )
        .optional()?;
    let Some(object_id) = object_id else {
        return Ok(None);
    };
    let object_id_for_update = object_id.clone();
    let object_id_for_delete = object_id.clone();
    let object_id_for_journal = object_id.clone();
    let mut manifest = super::rows::load_manifest_by_object_id(tx, &object_id)?
        .ok_or_else(|| MetadataError::ManifestNotFound(object_id.clone()))?;
    super::manifests::enforce_delete_conditionals_snapshot(
        if_match,
        if_match_last_modified_time,
        if_match_size,
        &manifest,
    )?;
    let tombstoned_at = timestamp_now()?;
    manifest.commit_state = CommitState::Tombstoned;
    crate::durable::enqueue_manifest_cleanup(tx, &manifest)?;
    let manifest_json = serde_json::to_string(&manifest)?;
    tx.execute(
        "UPDATE object_manifests SET commit_state='tombstoned',manifest_json=?2,tombstoned_at=?3 WHERE object_id=?1",
        params![object_id_for_update, manifest_json, tombstoned_at],
    )?;
    tx.execute(
        "DELETE FROM active_objects WHERE bucket=?1 AND object_key=?2 AND object_id=?3",
        params![bucket, object_key, object_id_for_delete],
    )?;
    tx.execute(
        "DELETE FROM recovery_markers WHERE object_id=?1",
        [object_id_for_delete.as_str()],
    )?;
    tx.execute(
        "UPDATE operation_journal SET state='tombstoned',error=?2,updated_at=?3 WHERE object_id=?1",
        params![object_id_for_journal, reason, tombstoned_at],
    )?;
    Ok(Some(manifest))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::manifest::CommittedManifestArgs;
    use crate::metadata::OperationKind;
    use s3s::dto::{ETag, ETagCondition};
    use time::OffsetDateTime;

    fn sample_manifest(
        bucket: &str,
        key: &str,
        whole_object: &str,
    ) -> crate::manifest::ObjectManifest {
        crate::manifest::ObjectManifest::committed(CommittedManifestArgs {
            bucket: bucket.to_string(),
            key: key.to_string(),
            content_length: 3,
            content_type: "text/plain".to_string(),
            checksum_algorithm: "sha256".to_string(),
            whole_object: whole_object.to_string(),
            peer_id: "peer".to_string(),
            message_id: 1,
        })
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
        store
            .create_user("u10", "backup", "hash", "superadmin", "")
            .expect("backup superadmin");
        store.delete_user(&user.id).expect("delete");
        assert!(store.get_session("c2").expect("session").is_none());
        assert_eq!(store.user_count().expect("count"), 1);
    }

    #[test]
    fn delete_active_key_rechecks_conditionals_in_transaction() {
        let store = MetadataStore::open_in_memory().expect("open");
        let bucket = store
            .create_bucket(crate::metadata::BucketRecord {
                name: "bucket".to_string(),
                created_at: OffsetDateTime::now_utc(),
                deleted_at: None,
                versioning_enabled: false,
                object_locking_enabled: false,
            })
            .expect("bucket");

        let original = sample_manifest(&bucket.name, "key.txt", "abcd");
        let original_op = store
            .stage_manifest(OperationKind::Put, original.clone())
            .expect("stage original");
        store.commit_manifest(original_op).expect("commit original");

        let replacement = sample_manifest(&bucket.name, "key.txt", "efgh");
        let replacement_op = store
            .stage_manifest(OperationKind::Put, replacement.clone())
            .expect("stage replacement");
        store
            .commit_manifest(replacement_op)
            .expect("commit replacement");

        let stale_condition = ETagCondition::ETag(ETag::Strong("abcd".to_string()));
        let error = store
            .delete_active_key(
                &bucket.name,
                "key.txt",
                "deleted via S3",
                Some(&stale_condition),
                None,
                None,
            )
            .expect_err("stale delete should fail");
        assert!(
            matches!(error, MetadataError::PreconditionFailed(message) if message.contains("if-match"))
        );

        let active = store
            .get_active_manifest(&bucket.name, "key.txt")
            .expect("active");
        assert_eq!(active.expect("present").checksum.whole_object, "efgh");
    }

    #[test]
    fn delete_empty_folder_rejects_active_descendants() {
        let store = MetadataStore::open_in_memory().expect("open");
        let bucket = store
            .create_bucket(crate::metadata::BucketRecord {
                name: "bucket".to_string(),
                created_at: OffsetDateTime::now_utc(),
                deleted_at: None,
                versioning_enabled: false,
                object_locking_enabled: false,
            })
            .expect("bucket");

        for manifest in [
            sample_manifest(&bucket.name, "docs/", "marker"),
            sample_manifest(&bucket.name, "docs/report.txt", "report"),
        ] {
            let operation = store
                .stage_manifest(OperationKind::Put, manifest)
                .expect("stage");
            store.commit_manifest(operation).expect("commit");
        }

        let error = store
            .delete_empty_folder(&bucket.name, "docs/", "deleted via admin")
            .expect_err("non-empty folder must not be reported as deleted");
        assert!(matches!(error, MetadataError::FolderNotEmpty(path) if path == "docs/"));
        assert!(
            store
                .get_active_manifest(&bucket.name, "docs/report.txt")
                .expect("child")
                .is_some()
        );

        store
            .delete_active_key(
                &bucket.name,
                "docs/report.txt",
                "deleted via admin",
                None,
                None,
                None,
            )
            .expect("delete child");
        assert!(
            store
                .delete_empty_folder(&bucket.name, "docs/", "deleted via admin")
                .expect("delete empty folder")
                .is_some()
        );
        assert!(
            store
                .get_active_manifest(&bucket.name, "docs/")
                .expect("folder marker")
                .is_none()
        );
    }
}
