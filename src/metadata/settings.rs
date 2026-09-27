use super::rows::timestamp_now;
use super::{MetadataError, MetadataStore, TrafficCounterKind, TrafficTotals};
use rusqlite::{OptionalExtension, params};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use uuid::Uuid;

const CHUNK_SIZE_SETTING: &str = "telegram_chunk_size";
pub(crate) const DOWNLOAD_PREFETCH_CHUNKS_SETTING: &str = "telegram_download_prefetch_chunks";
const RECOVERY_VERIFY_ENABLED_SETTING: &str = "telegram_recovery_verify_enabled";
const RECOVERY_VERIFY_INTERVAL_SETTING: &str = "telegram_recovery_verify_interval_secs";
const RECOVERY_VERIFY_CHUNKS_SETTING: &str = "telegram_recovery_verify_chunks";
const TELEGRAM_ACCOUNT_PHONE_SETTING: &str = "telegram_account_phone";

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct TelegramBootstrapSettings {
    pub telegram_api_id: Option<String>,
    pub telegram_api_hash: Option<String>,
    pub telegram_session_path: Option<String>,
    pub telegram_storage_chat_id: Option<String>,
    pub telegram_proxy_url: Option<String>,
    pub telegram_proxy_username: Option<String>,
    pub telegram_proxy_password: Option<String>,
    pub telegram_proxy_mode: Option<String>,
}

impl MetadataStore {
    pub(crate) fn traffic_totals(&self) -> Result<TrafficTotals, MetadataError> {
        self.with_connection(|connection| {
            let values = connection.query_row(
                "SELECT client_upload_bytes, client_download_bytes, telegram_upload_bytes, telegram_download_bytes FROM traffic_totals WHERE id=1",
                [],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, String>(2)?,
                        row.get::<_, String>(3)?,
                    ))
                },
            )?;
            Ok(TrafficTotals {
                client_upload_bytes: parse_traffic_total("client_upload_bytes", &values.0)?,
                client_download_bytes: parse_traffic_total("client_download_bytes", &values.1)?,
                telegram_upload_bytes: parse_traffic_total("telegram_upload_bytes", &values.2)?,
                telegram_download_bytes: parse_traffic_total("telegram_download_bytes", &values.3)?,
            })
        })
    }

    pub(crate) fn increment_traffic_total(
        &self,
        kind: TrafficCounterKind,
        bytes: u64,
    ) -> Result<(), MetadataError> {
        self.with_connection(|connection| {
            let column = kind.column();
            let current: String = connection.query_row(
                &format!("SELECT {column} FROM traffic_totals WHERE id=1"),
                [],
                |row| row.get(0),
            )?;
            let current = parse_traffic_total(column, &current)?;
            let next = current.checked_add(bytes).ok_or_else(|| {
                MetadataError::InvalidManifest(format!("traffic total overflow for {column}"))
            })?;
            connection.execute(
                &format!("UPDATE traffic_totals SET {column}=?1, updated_at=?2 WHERE id=1"),
                params![next.to_string(), timestamp_now()?],
            )?;
            Ok(())
        })
    }

    pub fn telegram_chunk_size(&self) -> Result<Option<u64>, MetadataError> {
        self.with_connection(|connection| {
            let value: Option<String> = connection
                .query_row(
                    "SELECT value FROM app_settings WHERE key=?1",
                    [CHUNK_SIZE_SETTING],
                    |row| row.get(0),
                )
                .optional()?;
            value
                .map(|value| {
                    value.parse::<u64>().map_err(|_| {
                        MetadataError::InvalidManifest("invalid stored chunk size".into())
                    })
                })
                .transpose()
        })
    }

    pub fn set_telegram_chunk_size(&self, chunk_size: u64) -> Result<(), MetadataError> {
        self.with_connection(|connection| {
            connection.execute(
                "INSERT INTO app_settings(key,value,updated_at) VALUES(?1,?2,?3) ON CONFLICT(key) DO UPDATE SET value=excluded.value, updated_at=excluded.updated_at",
                params![CHUNK_SIZE_SETTING, chunk_size.to_string(), timestamp_now()?],
            )?;
            Ok(())
        })
    }

    pub fn telegram_download_prefetch_chunks(&self) -> Result<Option<u64>, MetadataError> {
        self.read_numeric_setting(DOWNLOAD_PREFETCH_CHUNKS_SETTING)
    }

    pub fn set_telegram_download_prefetch_chunks(&self, chunks: u64) -> Result<(), MetadataError> {
        self.set_numeric_setting(DOWNLOAD_PREFETCH_CHUNKS_SETTING, chunks)
    }

    pub fn telegram_recovery_verify_interval_secs(&self) -> Result<Option<u64>, MetadataError> {
        self.read_numeric_setting(RECOVERY_VERIFY_INTERVAL_SETTING)
    }

    pub fn telegram_recovery_verify_enabled(&self) -> Result<Option<bool>, MetadataError> {
        self.read_bool_setting(RECOVERY_VERIFY_ENABLED_SETTING)
    }

    pub fn set_telegram_recovery_verify_enabled(&self, enabled: bool) -> Result<(), MetadataError> {
        self.set_bool_setting(RECOVERY_VERIFY_ENABLED_SETTING, enabled)
    }

    pub fn set_telegram_recovery_verify_interval_secs(
        &self,
        interval_secs: u64,
    ) -> Result<(), MetadataError> {
        self.set_numeric_setting(RECOVERY_VERIFY_INTERVAL_SETTING, interval_secs)
    }

    pub fn telegram_recovery_verify_chunks(&self) -> Result<Option<u64>, MetadataError> {
        self.read_numeric_setting(RECOVERY_VERIFY_CHUNKS_SETTING)
    }

    pub fn set_telegram_recovery_verify_chunks(&self, chunks: u64) -> Result<(), MetadataError> {
        self.set_numeric_setting(RECOVERY_VERIFY_CHUNKS_SETTING, chunks)
    }

    fn read_numeric_setting(&self, key: &str) -> Result<Option<u64>, MetadataError> {
        self.with_connection(|connection| {
            let value: Option<String> = connection
                .query_row(
                    "SELECT value FROM app_settings WHERE key=?1",
                    [key],
                    |row| row.get(0),
                )
                .optional()?;
            value
                .map(|value| {
                    value.parse::<u64>().map_err(|_| {
                        MetadataError::InvalidManifest(format!("invalid stored setting {key}"))
                    })
                })
                .transpose()
        })
    }

    fn read_bool_setting(&self, key: &str) -> Result<Option<bool>, MetadataError> {
        self.with_connection(|connection| {
            let value: Option<String> = connection
                .query_row(
                    "SELECT value FROM app_settings WHERE key=?1",
                    [key],
                    |row| row.get(0),
                )
                .optional()?;
            value
                .map(|value| match value.as_str() {
                    "true" | "1" | "yes" | "on" => Ok(true),
                    "false" | "0" | "no" | "off" => Ok(false),
                    _ => Err(MetadataError::InvalidManifest(format!(
                        "invalid stored setting {key}"
                    ))),
                })
                .transpose()
        })
    }

    fn set_numeric_setting(&self, key: &str, value: u64) -> Result<(), MetadataError> {
        self.with_connection(|connection| {
            connection.execute(
                "INSERT INTO app_settings(key,value,updated_at) VALUES(?1,?2,?3) ON CONFLICT(key) DO UPDATE SET value=excluded.value, updated_at=excluded.updated_at",
                params![key, value.to_string(), timestamp_now()?],
            )?;
            Ok(())
        })
    }

    fn set_bool_setting(&self, key: &str, value: bool) -> Result<(), MetadataError> {
        self.with_connection(|connection| {
            connection.execute(
                "INSERT INTO app_settings(key,value,updated_at) VALUES(?1,?2,?3) ON CONFLICT(key) DO UPDATE SET value=excluded.value, updated_at=excluded.updated_at",
                params![key, value.to_string(), timestamp_now()?],
            )?;
            Ok(())
        })
    }

    pub fn set_object_recovery_marker(
        &self,
        object_id: Uuid,
        bucket: &str,
        object_key: &str,
        details_json: &str,
    ) -> Result<(), MetadataError> {
        self.with_connection(|connection| {
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
                VALUES (?1, ?2, ?3, ?4, 'recovery_required', ?5, ?6, ?6)
                ON CONFLICT(marker_key) DO UPDATE SET
                    object_id = excluded.object_id,
                    bucket = excluded.bucket,
                    object_key = excluded.object_key,
                    marker_state = excluded.marker_state,
                    details_json = excluded.details_json,
                    updated_at = excluded.updated_at
                "#,
                params![
                    format!("object:{object_id}"),
                    object_id.to_string(),
                    bucket,
                    object_key,
                    details_json,
                    now,
                ],
            )?;
            Ok(())
        })
    }

    pub fn object_recovery_marker(&self, object_id: Uuid) -> Result<Option<String>, MetadataError> {
        self.with_connection(|connection| {
            connection
                .query_row(
                    "SELECT details_json FROM recovery_markers WHERE marker_key=?1 AND marker_state='recovery_required'",
                    [format!("object:{object_id}")],
                    |row| row.get(0),
                )
                .optional()
                .map_err(MetadataError::from)
        })
    }

    pub fn set_telegram_account_phone(&self, phone: &str) -> Result<(), MetadataError> {
        let phone = phone.trim();
        if phone.is_empty() {
            return Err(MetadataError::InvalidManifest(
                "telegram account phone is required".to_string(),
            ));
        }
        self.with_connection(|connection| {
            let tx = connection.transaction()?;
            tx.execute(
                "INSERT INTO app_settings(key,value,updated_at) VALUES(?1,?2,?3) ON CONFLICT(key) DO UPDATE SET value=excluded.value, updated_at=excluded.updated_at",
                params![TELEGRAM_ACCOUNT_PHONE_SETTING, phone, timestamp_now()?],
            )?;
            // The previous implementation stored only this irreversible value.
            // It cannot be converted back into the phone number, so remove it
            // whenever the account is re-authorized and the normal value is
            // available.
            tx.execute(
                "DELETE FROM app_settings WHERE key='telegram_account_phone_hash'",
                [],
            )?;
            tx.commit()?;
            Ok(())
        })
    }

    pub fn telegram_account_phone(&self) -> Result<Option<String>, MetadataError> {
        self.with_connection(|connection| {
            Ok(connection
                .query_row(
                    "SELECT value FROM app_settings WHERE key=?1",
                    [TELEGRAM_ACCOUNT_PHONE_SETTING],
                    |row| row.get(0),
                )
                .optional()?)
        })
    }

    pub fn telegram_account_phone_matches(&self, phone: &str) -> Result<bool, MetadataError> {
        let normalized = normalize_phone(phone);
        self.with_connection(|connection| {
            let stored: Option<String> = connection
                .query_row(
                    "SELECT value FROM app_settings WHERE key=?1",
                    [TELEGRAM_ACCOUNT_PHONE_SETTING],
                    |row| row.get(0),
                )
                .optional()?;
            Ok(stored.is_some_and(|value| normalize_phone(&value) == normalized))
        })
    }

    pub fn telegram_bootstrap_settings(
        &self,
    ) -> Result<Option<TelegramBootstrapSettings>, MetadataError> {
        self.with_connection(|connection| {
            let json: Option<String> = connection
                .query_row(
                    r#"
                    SELECT value
                    FROM app_settings
                    WHERE key = 'telegram_bootstrap'
                    "#,
                    [],
                    |row| row.get(0),
                )
                .optional()?;
            match json {
                Some(json) => Ok(Some(serde_json::from_str::<TelegramBootstrapSettings>(
                    &json,
                )?)),
                None => Ok(None),
            }
        })
    }

    pub fn set_telegram_bootstrap_settings(
        &self,
        settings: &TelegramBootstrapSettings,
    ) -> Result<(), MetadataError> {
        let json = serde_json::to_string(settings)?;
        self.with_connection(|connection| {
            let tx = connection.transaction()?;
            let connection_id: Option<String> = tx
                .query_row(
                    "SELECT value FROM app_settings WHERE key='telegram_active_connection_id'",
                    [],
                    |row| row.get(0),
                )
                .optional()?;
            let connection_id = if let Some(connection_id) = connection_id {
                connection_id
            } else {
                let connection_id = Uuid::new_v4().to_string();
                tx.execute(
                    "INSERT INTO app_settings(key,value,updated_at) VALUES('telegram_active_connection_id',?1,?2)",
                    params![&connection_id, timestamp_now()?],
                )?;
                connection_id
            };
            for table in ["buckets", "object_manifests", "multipart_uploads", "transfer_jobs"] {
                tx.execute(
                    &format!("UPDATE {table} SET connection_id=?1 WHERE connection_id='legacy'"),
                    [&connection_id],
                )?;
            }
            tx.execute(
                r#"
                INSERT INTO app_settings (key, value, updated_at)
                VALUES ('telegram_bootstrap', ?1, ?2)
                ON CONFLICT(key) DO UPDATE SET
                    value = excluded.value,
                    updated_at = excluded.updated_at
                "#,
                params![json, timestamp_now()?],
            )?;
            tx.commit()?;
            Ok(())
        })
    }

    pub fn clear_telegram_bootstrap_settings(&self) -> Result<(), MetadataError> {
        self.with_connection(|connection| {
            connection.execute(
                "DELETE FROM app_settings WHERE key='telegram_bootstrap'",
                [],
            )?;
            Ok(())
        })
    }

    pub fn active_connection_id(&self) -> Result<Option<String>, MetadataError> {
        self.with_connection(|connection| {
            connection
                .query_row(
                    "SELECT value FROM app_settings WHERE key='telegram_active_connection_id'",
                    [],
                    |row| row.get(0),
                )
                .optional()
                .map_err(MetadataError::from)
        })
    }

    pub fn clear_active_connection_id(&self) -> Result<(), MetadataError> {
        self.with_connection(|connection| {
            connection.execute(
                "DELETE FROM app_settings WHERE key='telegram_active_connection_id'",
                [],
            )?;
            Ok(())
        })
    }

    /// Recovery issues an operator has reviewed and chosen to stop counting,
    /// keyed by [`crate::object_format::RecoveryIssue::fingerprint`].
    pub fn recovery_acknowledgements(&self) -> Result<RecoveryAcknowledgements, MetadataError> {
        self.with_connection(|connection| {
            let json: Option<String> = connection
                .query_row(
                    r#"
                    SELECT value
                    FROM app_settings
                    WHERE key = 'recovery_acknowledged'
                    "#,
                    [],
                    |row| row.get(0),
                )
                .optional()?;
            match json {
                Some(json) => Ok(serde_json::from_str::<RecoveryAcknowledgements>(&json)?),
                None => Ok(RecoveryAcknowledgements::default()),
            }
        })
    }

    pub fn set_recovery_acknowledgements(
        &self,
        acknowledgements: &RecoveryAcknowledgements,
    ) -> Result<(), MetadataError> {
        let json = serde_json::to_string(acknowledgements)?;
        self.with_connection(|connection| {
            let tx = connection.transaction()?;
            tx.execute(
                r#"
                INSERT INTO app_settings (key, value, updated_at)
                VALUES ('recovery_acknowledged', ?1, ?2)
                ON CONFLICT(key) DO UPDATE SET
                    value = excluded.value,
                    updated_at = excluded.updated_at
                "#,
                params![json, timestamp_now()?],
            )?;
            tx.commit()?;
            Ok(())
        })
    }
}

fn parse_traffic_total(name: &str, value: &str) -> Result<u64, MetadataError> {
    value
        .parse::<u64>()
        .map_err(|_| MetadataError::InvalidManifest(format!("invalid stored traffic total {name}")))
}

#[cfg(test)]
mod tests {
    use rusqlite::OptionalExtension;
    use uuid::Uuid;

    use super::MetadataStore;

    #[test]
    fn chunk_size_setting_round_trips_without_schema_changes() {
        let store = MetadataStore::open_in_memory().expect("metadata");
        assert_eq!(store.telegram_chunk_size().expect("read"), None);
        store
            .set_telegram_chunk_size(8 * 1024 * 1024)
            .expect("write");
        assert_eq!(
            store.telegram_chunk_size().expect("read"),
            Some(8 * 1024 * 1024)
        );
        assert_eq!(store.schema_version().expect("schema"), 14);
    }

    #[test]
    fn recovery_verification_settings_and_marker_round_trip() {
        let store = MetadataStore::open_in_memory().expect("metadata");
        assert_eq!(
            store.telegram_recovery_verify_enabled().expect("enabled"),
            None
        );
        store
            .set_telegram_recovery_verify_enabled(false)
            .expect("enabled write");
        assert_eq!(
            store
                .telegram_recovery_verify_interval_secs()
                .expect("interval"),
            None
        );
        store
            .set_telegram_recovery_verify_interval_secs(600)
            .expect("interval write");
        store
            .set_telegram_recovery_verify_chunks(3)
            .expect("chunks write");
        assert_eq!(
            store
                .telegram_recovery_verify_interval_secs()
                .expect("interval"),
            Some(600)
        );
        assert_eq!(
            store
                .telegram_recovery_verify_enabled()
                .expect("enabled after migration"),
            Some(false)
        );
        assert_eq!(
            store.telegram_recovery_verify_chunks().expect("chunks"),
            Some(3)
        );

        let object_id = Uuid::new_v4();
        store
            .set_object_recovery_marker(object_id, "bucket", "key", "[]")
            .expect("marker write");
        assert_eq!(
            store
                .object_recovery_marker(object_id)
                .expect("marker read"),
            Some("[]".to_string())
        );

        store.migrate().expect("idempotent migration");
        assert_eq!(store.schema_version().expect("schema"), 14);
        assert_eq!(
            store
                .telegram_recovery_verify_interval_secs()
                .expect("interval after migration"),
            Some(600)
        );
        assert_eq!(
            store
                .telegram_recovery_verify_chunks()
                .expect("chunks after migration"),
            Some(3)
        );
        assert_eq!(
            store
                .object_recovery_marker(object_id)
                .expect("marker after migration"),
            Some("[]".to_string())
        );
    }

    #[test]
    fn download_prefetch_setting_defaults_and_round_trips() {
        let store = MetadataStore::open_in_memory().expect("metadata");
        assert_eq!(
            store
                .telegram_download_prefetch_chunks()
                .expect("prefetch read"),
            Some(1)
        );
        store
            .set_telegram_download_prefetch_chunks(4)
            .expect("prefetch write");
        assert_eq!(
            store
                .telegram_download_prefetch_chunks()
                .expect("prefetch read"),
            Some(4)
        );
        store.migrate().expect("idempotent migration");
        assert_eq!(
            store
                .telegram_download_prefetch_chunks()
                .expect("prefetch after migration"),
            Some(4)
        );
    }

    #[test]
    fn account_phone_round_trips_as_plain_text_and_cleans_legacy_hash() {
        let store = MetadataStore::open_in_memory().expect("metadata");
        store
            .with_connection(|connection| {
                connection.execute(
                    "INSERT INTO app_settings(key,value,updated_at) VALUES('telegram_account_phone_hash','legacy-hash','now')",
                    [],
                )?;
                Ok(())
            })
            .expect("legacy hash");

        store
            .set_telegram_account_phone(" +1 (555) 123-4567 ")
            .expect("write phone");
        assert_eq!(
            store.telegram_account_phone().expect("read phone"),
            Some("+1 (555) 123-4567".to_string())
        );
        assert!(
            store
                .telegram_account_phone_matches("+15551234567")
                .expect("match phone")
        );
        assert!(
            store
                .with_connection(|connection| {
                    Ok(connection
                    .query_row(
                        "SELECT value FROM app_settings WHERE key='telegram_account_phone_hash'",
                        [],
                        |row| row.get::<_, String>(0),
                    )
                    .optional()?
                    .is_none())
                })
                .expect("legacy hash removed")
        );
    }
}

fn normalize_phone(phone: &str) -> String {
    phone.chars().filter(char::is_ascii_digit).collect()
}

/// Fingerprint -> who acknowledged it and when.
pub type RecoveryAcknowledgements = BTreeMap<String, RecoveryAck>;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RecoveryAck {
    /// RFC 3339 timestamp of the acknowledgement.
    pub at: String,
    /// Operator username that acknowledged the issue.
    pub by: String,
}
