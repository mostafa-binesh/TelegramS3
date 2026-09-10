use super::rows::timestamp_now;
use super::{MetadataError, MetadataStore};
use rusqlite::{OptionalExtension, params};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use uuid::Uuid;

const CHUNK_SIZE_SETTING: &str = "telegram_chunk_size";
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

#[cfg(test)]
mod tests {
    use rusqlite::OptionalExtension;

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
        assert_eq!(store.schema_version().expect("schema"), 11);
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
