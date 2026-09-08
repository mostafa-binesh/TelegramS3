use super::{BucketRecord, DbUser, MetadataError, OperationKind};
use crate::manifest::{CommitState, ObjectManifest};
use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize};
use time::OffsetDateTime;
use time::format_description::well_known::Rfc3339;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct JournalDetails {
    pub operation_id: String,
    pub object_id: String,
    pub bucket: String,
    pub object_key: String,
    pub operation_kind: OperationKind,
    pub state: String,
    pub reason: Option<String>,
}

pub(crate) fn row_user(
    connection: &Connection,
    sql: &str,
    params: &[&dyn rusqlite::ToSql],
) -> Result<Option<DbUser>, MetadataError> {
    let mut stmt = connection.prepare(sql)?;
    let mut rows = stmt.query(params)?;
    let row = match rows.next()? {
        Some(row) => row,
        None => return Ok(None),
    };
    Ok(Some(read_user_row(row)?))
}

pub(crate) fn read_user_row(row: &rusqlite::Row<'_>) -> Result<DbUser, MetadataError> {
    Ok(DbUser {
        id: row.get(0)?,
        username: row.get(1)?,
        password_hash: row.get(2)?,
        role: row.get(3)?,
        display_name: row.get(4)?,
        disabled: row.get::<_, i64>(5)? != 0,
        token_version: row.get(6)?,
        created_at: parse_timestamp(&row.get::<_, String>(7)?, 7)?,
        updated_at: parse_timestamp(&row.get::<_, String>(8)?, 8)?,
    })
}

pub(crate) fn parse_timestamp(value: &str, index: usize) -> Result<OffsetDateTime, MetadataError> {
    OffsetDateTime::parse(value, &Rfc3339)
        .map_err(|error| {
            rusqlite::Error::FromSqlConversionFailure(
                index,
                rusqlite::types::Type::Text,
                Box::new(error),
            )
        })
        .map_err(MetadataError::Sqlite)
}

pub(crate) fn load_manifest_by_object_id<T>(
    connection: &T,
    object_id: &str,
) -> Result<Option<ObjectManifest>, MetadataError>
where
    T: QueryRowExt,
{
    let manifest_json: Option<String> = connection
        .query_row(
            "SELECT manifest_json FROM object_manifests WHERE object_id = ?1",
            params![object_id],
            |row| row.get(0),
        )
        .optional()?;
    match manifest_json {
        Some(json) => {
            let manifest = serde_json::from_str::<ObjectManifest>(&json)?;
            manifest
                .validate()
                .map_err(MetadataError::InvalidManifest)?;
            Ok(Some(manifest))
        }
        None => Ok(None),
    }
}

pub(crate) fn load_journal_entry<T>(
    connection: &T,
    operation_id: &str,
) -> Result<Option<JournalDetails>, MetadataError>
where
    T: QueryRowExt,
{
    let journal_json: Option<String> = connection
        .query_row(
            "SELECT manifest_json FROM operation_journal WHERE operation_id = ?1",
            params![operation_id],
            |row| row.get(0),
        )
        .optional()?;
    match journal_json {
        Some(json) => Ok(Some(serde_json::from_str::<JournalDetails>(&json)?)),
        None => Ok(None),
    }
}

pub(crate) fn load_bucket_record<T>(
    connection: &T,
    bucket: &str,
    include_deleted: bool,
) -> Result<Option<BucketRecord>, MetadataError>
where
    T: QueryRowExt,
{
    let mut sql = String::from(
        r#"
        SELECT name, created_at, deleted_at, versioning_enabled, object_locking_enabled
        FROM buckets
        WHERE name = ?1
        "#,
    );
    if !include_deleted {
        sql.push_str(" AND deleted_at IS NULL");
    }
    let record = connection
        .query_row(&sql, params![bucket], |row| {
            let created_at = row.get::<_, String>(1)?;
            let deleted_at = row.get::<_, Option<String>>(2)?;
            Ok(BucketRecord {
                name: row.get(0)?,
                created_at: OffsetDateTime::parse(&created_at, &Rfc3339).map_err(|error| {
                    rusqlite::Error::FromSqlConversionFailure(
                        1,
                        rusqlite::types::Type::Text,
                        Box::new(error),
                    )
                })?,
                deleted_at: match deleted_at {
                    Some(value) => {
                        Some(OffsetDateTime::parse(&value, &Rfc3339).map_err(|error| {
                            rusqlite::Error::FromSqlConversionFailure(
                                2,
                                rusqlite::types::Type::Text,
                                Box::new(error),
                            )
                        })?)
                    }
                    None => None,
                },
                versioning_enabled: row.get::<_, i64>(3)? != 0,
                object_locking_enabled: row.get::<_, i64>(4)? != 0,
            })
        })
        .optional()?;
    Ok(record)
}

pub(crate) fn manifest_json_with_state(
    manifest: &ObjectManifest,
    commit_state: CommitState,
) -> Result<String, MetadataError> {
    let mut manifest = manifest.clone();
    manifest.commit_state = commit_state;
    Ok(serde_json::to_string(&manifest)?)
}

pub(crate) fn timestamp_now() -> Result<String, MetadataError> {
    Ok(OffsetDateTime::now_utc().format(&Rfc3339)?)
}

pub(crate) fn parse_rfc3339_timestamp(value: &str) -> Result<OffsetDateTime, MetadataError> {
    OffsetDateTime::parse(value, &Rfc3339).map_err(|error| {
        MetadataError::InvalidManifest(format!("invalid timestamp {value}: {error}"))
    })
}

pub(crate) fn count_rows(connection: &Connection, sql: &str) -> Result<u64, MetadataError> {
    Ok(connection.query_row(sql, [], |row| row.get::<_, u64>(0))?)
}

pub(crate) fn count_rows_tx(
    connection: &rusqlite::Transaction<'_>,
    sql: &str,
) -> Result<u64, MetadataError> {
    Ok(connection.query_row(sql, [], |row| row.get::<_, u64>(0))?)
}

pub(crate) trait QueryRowExt {
    fn query_row<T, P, F>(&self, sql: &str, params: P, f: F) -> Result<T, rusqlite::Error>
    where
        P: rusqlite::Params,
        F: FnOnce(&rusqlite::Row<'_>) -> Result<T, rusqlite::Error>;
}

impl QueryRowExt for Connection {
    fn query_row<T, P, F>(&self, sql: &str, params: P, f: F) -> Result<T, rusqlite::Error>
    where
        P: rusqlite::Params,
        F: FnOnce(&rusqlite::Row<'_>) -> Result<T, rusqlite::Error>,
    {
        let mut statement = self.prepare(sql)?;
        statement.query_row(params, f)
    }
}

impl<'conn> QueryRowExt for rusqlite::Transaction<'conn> {
    fn query_row<T, P, F>(&self, sql: &str, params: P, f: F) -> Result<T, rusqlite::Error>
    where
        P: rusqlite::Params,
        F: FnOnce(&rusqlite::Row<'_>) -> Result<T, rusqlite::Error>,
    {
        let mut statement = self.prepare(sql)?;
        statement.query_row(params, f)
    }
}
