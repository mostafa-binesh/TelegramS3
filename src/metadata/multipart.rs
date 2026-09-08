use super::rows::timestamp_now;
use super::{MetadataError, MetadataStore};
use crate::multipart::{MultipartPart, MultipartSession, MultipartState};
use rusqlite::{OptionalExtension, params};
use time::OffsetDateTime;
use uuid::Uuid;

impl MetadataStore {
    pub fn create_multipart_session(
        &self,
        session: MultipartSession,
    ) -> Result<MultipartSession, MetadataError> {
        let upload_id = session.upload_id.to_string();
        let bucket = session.bucket.clone();
        let object_key = session.key.clone();
        let state = session.state.as_str().to_string();
        let created_at = timestamp_now()?;
        let updated_at = created_at.clone();
        let session_json = serde_json::to_string(&session)?;

        self.with_connection(|connection| {
            let tx = connection.transaction()?;
            tx.execute(
                r#"
                INSERT INTO multipart_uploads (
                    upload_id,
                    bucket,
                    object_key,
                    state,
                    session_json,
                    created_at,
                    updated_at
                )
                VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
                ON CONFLICT(upload_id) DO UPDATE SET
                    bucket = excluded.bucket,
                    object_key = excluded.object_key,
                    state = excluded.state,
                    session_json = excluded.session_json,
                    updated_at = excluded.updated_at
                "#,
                params![
                    upload_id,
                    bucket,
                    object_key,
                    state,
                    session_json,
                    created_at,
                    updated_at
                ],
            )?;
            tx.commit()?;
            Ok(session)
        })
    }

    pub fn get_multipart_session(
        &self,
        upload_id: Uuid,
    ) -> Result<Option<MultipartSession>, MetadataError> {
        let upload_id = upload_id.to_string();
        self.with_connection(|connection| {
            let json: Option<String> = connection
                .query_row(
                    "SELECT session_json FROM multipart_uploads WHERE upload_id = ?1",
                    params![upload_id],
                    |row| row.get(0),
                )
                .optional()?;
            match json {
                Some(json) => Ok(Some(serde_json::from_str::<MultipartSession>(&json)?)),
                None => Ok(None),
            }
        })
    }

    pub fn list_multipart_sessions(
        &self,
        bucket: Option<&str>,
        prefix: Option<&str>,
    ) -> Result<Vec<MultipartSession>, MetadataError> {
        self.with_connection(|connection| {
            let mut sql = String::from(
                r#"
                SELECT session_json
                FROM multipart_uploads
                WHERE state IN ('initiated', 'uploading', 'completing', 'recovery_required')
                "#,
            );
            if bucket.is_some() {
                sql.push_str(" AND bucket = ?1");
            }
            if prefix.is_some() {
                if bucket.is_some() {
                    sql.push_str(" AND object_key LIKE ?2 || '%'");
                } else {
                    sql.push_str(" AND object_key LIKE ?1 || '%'");
                }
            }
            sql.push_str(" ORDER BY object_key ASC, upload_id ASC");

            let mut stmt = connection.prepare(&sql)?;
            let mut sessions = Vec::new();
            match (bucket, prefix) {
                (Some(bucket), Some(prefix)) => {
                    let mut rows = stmt.query(params![bucket, prefix])?;
                    while let Some(row) = rows.next()? {
                        let json = row.get::<_, String>(0)?;
                        sessions.push(serde_json::from_str::<MultipartSession>(&json)?);
                    }
                }
                (Some(bucket), None) => {
                    let mut rows = stmt.query(params![bucket])?;
                    while let Some(row) = rows.next()? {
                        let json = row.get::<_, String>(0)?;
                        sessions.push(serde_json::from_str::<MultipartSession>(&json)?);
                    }
                }
                (None, Some(prefix)) => {
                    let mut rows = stmt.query(params![prefix])?;
                    while let Some(row) = rows.next()? {
                        let json = row.get::<_, String>(0)?;
                        sessions.push(serde_json::from_str::<MultipartSession>(&json)?);
                    }
                }
                (None, None) => {
                    let mut rows = stmt.query([])?;
                    while let Some(row) = rows.next()? {
                        let json = row.get::<_, String>(0)?;
                        sessions.push(serde_json::from_str::<MultipartSession>(&json)?);
                    }
                }
            }
            Ok(sessions)
        })
    }

    pub fn update_multipart_session_state(
        &self,
        upload_id: Uuid,
        state: MultipartState,
    ) -> Result<MultipartSession, MetadataError> {
        let upload_id_string = upload_id.to_string();
        self.with_connection(|connection| {
            let tx = connection.transaction()?;
            let json: Option<String> = tx
                .query_row(
                    "SELECT session_json FROM multipart_uploads WHERE upload_id = ?1",
                    params![upload_id_string.clone()],
                    |row| row.get(0),
                )
                .optional()?;
            let mut session = match json {
                Some(json) => serde_json::from_str::<MultipartSession>(&json)?,
                None => {
                    return Err(MetadataError::MultipartSessionNotFound(
                        upload_id_string.clone(),
                    ));
                }
            };
            session.state = state;
            session.updated_at = OffsetDateTime::now_utc();
            let session_json = serde_json::to_string(&session)?;
            let updated_at = timestamp_now()?;
            tx.execute(
                r#"
                UPDATE multipart_uploads
                SET state = ?2,
                    session_json = ?3,
                    updated_at = ?4
                WHERE upload_id = ?1
                "#,
                params![upload_id_string, state.as_str(), session_json, updated_at],
            )?;
            tx.commit()?;
            Ok(session)
        })
    }

    pub fn delete_multipart_session(&self, upload_id: Uuid) -> Result<(), MetadataError> {
        let upload_id = upload_id.to_string();
        self.with_connection(|connection| {
            let tx = connection.transaction()?;
            tx.execute(
                "DELETE FROM multipart_uploads WHERE upload_id = ?1",
                params![upload_id],
            )?;
            tx.commit()?;
            Ok(())
        })
    }

    pub fn put_multipart_part(&self, part: MultipartPart) -> Result<MultipartPart, MetadataError> {
        let upload_id = part.upload_id.to_string();
        let part_json = serde_json::to_string(&part)?;
        let created_at = timestamp_now()?;
        self.with_connection(|connection| {
            let tx = connection.transaction()?;
            tx.execute(
                r#"
                INSERT INTO multipart_parts (
                    upload_id,
                    part_number,
                    part_json,
                    created_at
                )
                VALUES (?1, ?2, ?3, ?4)
                ON CONFLICT(upload_id, part_number) DO UPDATE SET
                    part_json = excluded.part_json
                "#,
                params![upload_id, part.part_number, part_json, created_at],
            )?;
            tx.execute(
                r#"
                UPDATE multipart_uploads
                SET state = CASE
                        WHEN state = 'initiated' THEN 'uploading'
                        ELSE state
                    END,
                    updated_at = ?2
                WHERE upload_id = ?1
                "#,
                params![part.upload_id.to_string(), timestamp_now()?],
            )?;
            tx.commit()?;
            Ok(part)
        })
    }

    pub fn list_multipart_parts(
        &self,
        upload_id: Uuid,
    ) -> Result<Vec<MultipartPart>, MetadataError> {
        let upload_id = upload_id.to_string();
        self.with_connection(|connection| {
            let mut stmt = connection.prepare(
                r#"
                SELECT part_json
                FROM multipart_parts
                WHERE upload_id = ?1
                ORDER BY part_number ASC
                "#,
            )?;
            let mut parts = Vec::new();
            let mut rows = stmt.query(params![upload_id])?;
            while let Some(row) = rows.next()? {
                let json = row.get::<_, String>(0)?;
                parts.push(serde_json::from_str::<MultipartPart>(&json)?);
            }
            Ok(parts)
        })
    }

    pub fn get_multipart_part(
        &self,
        upload_id: Uuid,
        part_number: u32,
    ) -> Result<Option<MultipartPart>, MetadataError> {
        let upload_id = upload_id.to_string();
        self.with_connection(|connection| {
            let json: Option<String> = connection
                .query_row(
                    "SELECT part_json FROM multipart_parts WHERE upload_id = ?1 AND part_number = ?2",
                    params![upload_id, part_number],
                    |row| row.get(0),
                )
                .optional()?;
            match json {
                Some(json) => Ok(Some(serde_json::from_str::<MultipartPart>(&json)?)),
                None => Ok(None),
            }
        })
    }

    pub fn delete_multipart_parts(&self, upload_id: Uuid) -> Result<(), MetadataError> {
        let upload_id = upload_id.to_string();
        self.with_connection(|connection| {
            let tx = connection.transaction()?;
            tx.execute(
                "DELETE FROM multipart_parts WHERE upload_id = ?1",
                params![upload_id],
            )?;
            tx.commit()?;
            Ok(())
        })
    }
}
