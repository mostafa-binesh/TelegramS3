use super::rows::{JournalDetails, count_rows_tx, timestamp_now};
use super::{MetadataError, MetadataStore};
use rusqlite::{Connection, params};
use std::collections::HashMap;
use std::str::FromStr;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RebuildReport {
    pub committed_rows: u64,
    pub active_rows: u64,
    pub staged_rows: u64,
    pub recovery_markers: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VerifyReport {
    pub expected_active_rows: u64,
    pub actual_active_rows: u64,
    pub mismatched_rows: u64,
    pub staged_rows: u64,
}

impl MetadataStore {
    pub fn rebuild_index(&self) -> Result<RebuildReport, MetadataError> {
        self.with_connection(rebuild_index_internal)
    }

    pub fn verify_index(&self) -> Result<VerifyReport, MetadataError> {
        self.with_connection(verify_index_internal)
    }

    pub fn startup_reconcile(&self) -> Result<RebuildReport, MetadataError> {
        self.rebuild_index()
    }
}

pub(crate) fn ensure_recovery_marker(
    store: &MetadataStore,
    details: &JournalDetails,
) -> Result<(), MetadataError> {
    store.with_connection(|connection| {
        let now = timestamp_now()?;
        connection.execute(
            r#"
            INSERT INTO recovery_markers (
                marker_key,
                object_id,
                bucket,
                object_key,
                marker_state,
                details_json,
                created_at,
                updated_at
            )
            VALUES (?1, ?2, ?3, ?4, 'staging', ?5, ?6, ?6)
            ON CONFLICT(marker_key) DO UPDATE SET
                object_id = excluded.object_id,
                bucket = excluded.bucket,
                object_key = excluded.object_key,
                marker_state = excluded.marker_state,
                details_json = excluded.details_json,
                updated_at = excluded.updated_at
            "#,
            params![
                format!("staging:{}", details.operation_id),
                details.object_id,
                details.bucket,
                details.object_key,
                serde_json::to_string(details)?,
                now,
            ],
        )?;
        Ok(())
    })
}

pub(crate) fn rebuild_index_internal(
    connection: &mut Connection,
) -> Result<RebuildReport, MetadataError> {
    let tx = connection.transaction()?;
    tx.execute("DELETE FROM active_objects", [])?;

    let mut committed_rows = 0_u64;
    let mut staged_rows = 0_u64;
    let mut recovery_markers = 0_u64;

    {
        let mut stmt = tx.prepare(
            r#"
            SELECT operation_id, object_id, bucket, object_key, operation_kind, state
            FROM operation_journal
            ORDER BY created_at ASC, operation_id ASC
            "#,
        )?;
        let entries = stmt.query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, String>(4)?,
                row.get::<_, String>(5)?,
            ))
        })?;
        for entry in entries {
            let (operation_id, object_id, bucket, object_key, operation_kind, state) = entry?;
            if state != "staging" {
                continue;
            }
            staged_rows += 1;
            recovery_markers += 1;
            let details = JournalDetails {
                operation_id: operation_id.clone(),
                object_id,
                bucket,
                object_key,
                operation_kind: super::OperationKind::from_str(&operation_kind)?,
                state,
                reason: Some("pending startup reconciliation".to_string()),
            };
            tx.execute(
                r#"
                INSERT INTO recovery_markers (
                    marker_key,
                    object_id,
                    bucket,
                    object_key,
                    marker_state,
                    details_json,
                    created_at,
                    updated_at
                )
                VALUES (?1, ?2, ?3, ?4, 'staging', ?5, ?6, ?6)
                ON CONFLICT(marker_key) DO UPDATE SET
                    object_id = excluded.object_id,
                    bucket = excluded.bucket,
                    object_key = excluded.object_key,
                    marker_state = excluded.marker_state,
                    details_json = excluded.details_json,
                    updated_at = excluded.updated_at
                "#,
                params![
                    format!("staging:{}", operation_id),
                    details.object_id,
                    details.bucket,
                    details.object_key,
                    serde_json::to_string(&details)?,
                    timestamp_now()?,
                ],
            )?;
        }
    }

    {
        let mut stmt = tx.prepare(
            r#"
            SELECT bucket, object_key, object_id
            FROM object_manifests
            WHERE commit_state = 'committed'
            ORDER BY committed_at ASC, object_id ASC
            "#,
        )?;
        let entries = stmt.query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
            ))
        })?;
        for entry in entries {
            let (bucket, object_key, object_id) = entry?;
            committed_rows += 1;
            tx.execute(
                r#"
                INSERT INTO active_objects (bucket, object_key, object_id, updated_at)
                VALUES (?1, ?2, ?3, ?4)
                ON CONFLICT(bucket, object_key) DO UPDATE SET
                    object_id = excluded.object_id,
                    updated_at = excluded.updated_at
                "#,
                params![bucket, object_key, object_id, timestamp_now()?],
            )?;
        }
    }

    let active_rows = count_rows_tx(&tx, "SELECT COUNT(*) FROM active_objects")?;
    tx.commit()?;

    Ok(RebuildReport {
        committed_rows,
        active_rows,
        staged_rows,
        recovery_markers,
    })
}

pub(crate) fn verify_index_internal(
    connection: &mut Connection,
) -> Result<VerifyReport, MetadataError> {
    let mut expected = HashMap::<(String, String), String>::new();
    let mut staged_rows = 0_u64;

    {
        let mut stmt = connection.prepare(
            r#"
            SELECT bucket, object_key, object_id, commit_state
            FROM object_manifests
            ORDER BY committed_at ASC, object_id ASC
            "#,
        )?;
        let entries = stmt.query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
            ))
        })?;
        for entry in entries {
            let (bucket, object_key, object_id, commit_state) = entry?;
            if commit_state == "staging" {
                staged_rows += 1;
                continue;
            }
            if commit_state == "committed" {
                expected.insert((bucket, object_key), object_id);
            }
        }
    }

    let mut actual = HashMap::<(String, String), String>::new();
    {
        let mut stmt =
            connection.prepare("SELECT bucket, object_key, object_id FROM active_objects")?;
        let entries = stmt.query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
            ))
        })?;
        for entry in entries {
            let (bucket, object_key, object_id) = entry?;
            actual.insert((bucket, object_key), object_id);
        }
    }

    let mut mismatched_rows = 0_u64;
    for (key, expected_object_id) in &expected {
        match actual.get(key) {
            Some(actual_object_id) if actual_object_id == expected_object_id => {}
            _ => mismatched_rows += 1,
        }
    }
    for key in actual.keys() {
        if !expected.contains_key(key) {
            mismatched_rows += 1;
        }
    }

    Ok(VerifyReport {
        expected_active_rows: expected.len() as u64,
        actual_active_rows: actual.len() as u64,
        mismatched_rows,
        staged_rows,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::OperationKind;
    use crate::manifest::CommittedManifestArgs;
    use tempfile::tempdir;

    fn sample_manifest(bucket: &str, key: &str) -> crate::manifest::ObjectManifest {
        crate::manifest::ObjectManifest::committed(CommittedManifestArgs {
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
    fn rebuild_restores_missing_active_rows() {
        let dir = tempdir().expect("tempdir");
        let path = dir.path().join("metadata.sqlite");
        let store = MetadataStore::open(&path).expect("open");
        let manifest = sample_manifest("bucket", "key.txt");
        let operation_id = store
            .stage_manifest(OperationKind::Put, manifest.clone())
            .expect("stage");
        store.commit_manifest(operation_id).expect("commit");

        store
            .with_connection(|connection| {
                connection.execute("DELETE FROM active_objects", [])?;
                Ok(())
            })
            .expect("corrupt");

        let report = store.rebuild_index().expect("rebuild");
        assert_eq!(report.active_rows, 1);
        assert_eq!(store.verify_index().expect("verify").mismatched_rows, 0);
        let active = store
            .get_active_manifest("bucket", "key.txt")
            .expect("fetch")
            .expect("visible");
        assert_eq!(active.object_id, manifest.object_id);
    }
}
