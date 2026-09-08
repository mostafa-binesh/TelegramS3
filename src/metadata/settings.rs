use super::rows::timestamp_now;
use super::{MetadataError, MetadataStore};
use rusqlite::{OptionalExtension, params};
use serde::{Deserialize, Serialize};

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
}
