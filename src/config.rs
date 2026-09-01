use std::env;
use std::fs;
use std::path::{Component, Path, PathBuf};
use thiserror::Error;

const DEFAULT_METADATA_PATH: &str = "data/metadata.sqlite";
const DEFAULT_DATA_DIR: &str = "data";

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AppConfig {
    pub telegram_api_id: Option<String>,
    pub telegram_api_hash: Option<String>,
    pub telegram_phone_number: Option<String>,
    pub telegram_session_path: Option<String>,
    pub telegram_storage_chat_id: Option<String>,
    pub telegram_proxy_url: Option<String>,
    pub telegram_proxy_username: Option<String>,
    pub telegram_proxy_password: Option<String>,
    pub telegram_proxy_mode: Option<String>,
    pub telegram_metadata_path: Option<String>,
    pub telegram_data_dir: Option<String>,
    pub telegram_chunk_size: Option<String>,
    pub telegram_connection_timeout_secs: Option<String>,
    pub telegram_request_timeout_secs: Option<String>,
    pub telegram_transfer_timeout_secs: Option<String>,
    pub telegram_retry_count: Option<String>,
    pub telegram_retry_backoff_ms: Option<String>,
    pub telegram_respect_flood_wait: Option<String>,
    pub telegram_s3_master_key: Option<String>,
    pub rustfs_access_key: Option<String>,
    pub rustfs_secret_key: Option<String>,
    pub telegram_admin_bootstrap_secret: Option<String>,
    pub telegram_admin_ui_dist_dir: Option<String>,
    pub telegram_s3_bind_addr: Option<String>,
    pub telegram_admin_bind_addr: Option<String>,
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum ConfigError {
    #[error("{0}")]
    Missing(&'static str),
    #[error("invalid value for {0}")]
    Invalid(&'static str),
    #[error("invalid value for {field}: {value}")]
    Parse { field: &'static str, value: String },
    #[error("path {path} is unsafe for {field}: {reason}")]
    PathUnsafe {
        field: &'static str,
        path: String,
        reason: &'static str,
    },
    #[error("path {path} has unsafe permissions for {field}: {reason}")]
    UnsafePermissions {
        field: &'static str,
        path: String,
        reason: &'static str,
    },
}

impl AppConfig {
    pub fn from_env() -> Self {
        Self {
            telegram_api_id: read("TELEGRAM_API_ID"),
            telegram_api_hash: read("TELEGRAM_API_HASH"),
            telegram_phone_number: read("TELEGRAM_PHONE_NUMBER"),
            telegram_session_path: read("TELEGRAM_SESSION_PATH"),
            telegram_storage_chat_id: read("TELEGRAM_STORAGE_CHAT_ID"),
            telegram_proxy_url: read("TELEGRAM_PROXY_URL"),
            telegram_proxy_username: read("TELEGRAM_PROXY_USERNAME"),
            telegram_proxy_password: read("TELEGRAM_PROXY_PASSWORD"),
            telegram_proxy_mode: read("TELEGRAM_PROXY_MODE"),
            telegram_metadata_path: read("TELEGRAM_METADATA_PATH"),
            telegram_data_dir: read("TELEGRAM_DATA_DIR"),
            telegram_chunk_size: read("TELEGRAM_CHUNK_SIZE"),
            telegram_connection_timeout_secs: read("TELEGRAM_CONNECTION_TIMEOUT_SECS"),
            telegram_request_timeout_secs: read("TELEGRAM_REQUEST_TIMEOUT_SECS"),
            telegram_transfer_timeout_secs: read("TELEGRAM_TRANSFER_TIMEOUT_SECS"),
            telegram_retry_count: read("TELEGRAM_RETRY_COUNT"),
            telegram_retry_backoff_ms: read("TELEGRAM_RETRY_BACKOFF_MS"),
            telegram_respect_flood_wait: read("TELEGRAM_FLOOD_WAIT_RESPECT"),
            telegram_s3_master_key: read("TELEGRAM_S3_MASTER_KEY"),
            rustfs_access_key: read("RUSTFS_ACCESS_KEY"),
            rustfs_secret_key: read("RUSTFS_SECRET_KEY"),
            telegram_admin_bootstrap_secret: read("TELEGRAM_ADMIN_BOOTSTRAP_SECRET"),
            telegram_admin_ui_dist_dir: read("TELEGRAM_ADMIN_UI_DIST_DIR"),
            telegram_s3_bind_addr: read("TELEGRAM_S3_BIND_ADDR"),
            telegram_admin_bind_addr: read("TELEGRAM_ADMIN_BIND_ADDR"),
        }
    }

    pub fn metadata_path(&self) -> PathBuf {
        self.telegram_metadata_path
            .as_deref()
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from(DEFAULT_METADATA_PATH))
    }

    pub fn data_dir(&self) -> PathBuf {
        self.telegram_data_dir
            .as_deref()
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from(DEFAULT_DATA_DIR))
    }

    pub fn chunk_size(&self) -> Result<u64, ConfigError> {
        parse_u64(
            "TELEGRAM_CHUNK_SIZE",
            self.telegram_chunk_size.as_deref(),
            1,
            2_000_000_000,
            1_048_576,
        )
    }

    pub fn connection_timeout_secs(&self) -> Result<u64, ConfigError> {
        parse_u64(
            "TELEGRAM_CONNECTION_TIMEOUT_SECS",
            self.telegram_connection_timeout_secs.as_deref(),
            1,
            86_400,
            30,
        )
    }

    pub fn request_timeout_secs(&self) -> Result<u64, ConfigError> {
        parse_u64(
            "TELEGRAM_REQUEST_TIMEOUT_SECS",
            self.telegram_request_timeout_secs.as_deref(),
            1,
            86_400,
            30,
        )
    }

    pub fn transfer_timeout_secs(&self) -> Result<u64, ConfigError> {
        parse_u64(
            "TELEGRAM_TRANSFER_TIMEOUT_SECS",
            self.telegram_transfer_timeout_secs.as_deref(),
            1,
            86_400,
            900,
        )
    }

    pub fn retry_count(&self) -> Result<u32, ConfigError> {
        parse_u32(
            "TELEGRAM_RETRY_COUNT",
            self.telegram_retry_count.as_deref(),
            1,
            1_000,
            5,
        )
    }

    pub fn retry_backoff_ms(&self) -> Result<u64, ConfigError> {
        parse_u64(
            "TELEGRAM_RETRY_BACKOFF_MS",
            self.telegram_retry_backoff_ms.as_deref(),
            1,
            600_000,
            500,
        )
    }

    pub fn respect_flood_wait(&self) -> Result<bool, ConfigError> {
        parse_bool(
            "TELEGRAM_FLOOD_WAIT_RESPECT",
            self.telegram_respect_flood_wait.as_deref(),
            true,
        )
    }

    pub fn proxy_mode(&self) -> String {
        self.telegram_proxy_mode
            .as_deref()
            .unwrap_or("auto")
            .to_string()
    }

    pub fn s3_bind_addr(&self) -> Result<std::net::SocketAddr, ConfigError> {
        let value = self
            .telegram_s3_bind_addr
            .as_deref()
            .unwrap_or("127.0.0.1:9000");
        value.parse().map_err(|_| ConfigError::Parse {
            field: "TELEGRAM_S3_BIND_ADDR",
            value: value.to_string(),
        })
    }

    pub fn admin_bind_addr(&self) -> Result<std::net::SocketAddr, ConfigError> {
        let value = self
            .telegram_admin_bind_addr
            .as_deref()
            .unwrap_or("127.0.0.1:9001");
        value.parse().map_err(|_| ConfigError::Parse {
            field: "TELEGRAM_ADMIN_BIND_ADDR",
            value: value.to_string(),
        })
    }

    pub fn admin_ui_dist_dir(&self) -> PathBuf {
        self.telegram_admin_ui_dist_dir
            .as_deref()
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("frontend/dist"))
    }

    pub fn validate(&self) -> Result<(), ConfigError> {
        if self.telegram_api_id.as_deref().is_none_or(str::is_empty) {
            return Err(ConfigError::Missing("TELEGRAM_API_ID"));
        }
        if self.telegram_api_hash.as_deref().is_none_or(str::is_empty) {
            return Err(ConfigError::Missing("TELEGRAM_API_HASH"));
        }
        if self
            .telegram_session_path
            .as_deref()
            .is_none_or(str::is_empty)
        {
            return Err(ConfigError::Missing("TELEGRAM_SESSION_PATH"));
        }
        if self
            .telegram_storage_chat_id
            .as_deref()
            .is_none_or(str::is_empty)
        {
            return Err(ConfigError::Missing("TELEGRAM_STORAGE_CHAT_ID"));
        }
        if self
            .telegram_s3_master_key
            .as_deref()
            .is_none_or(str::is_empty)
        {
            return Err(ConfigError::Missing("TELEGRAM_S3_MASTER_KEY"));
        }
        if self.rustfs_access_key.as_deref().is_none_or(str::is_empty) {
            return Err(ConfigError::Missing("RUSTFS_ACCESS_KEY"));
        }
        if self.rustfs_secret_key.as_deref().is_none_or(str::is_empty) {
            return Err(ConfigError::Missing("RUSTFS_SECRET_KEY"));
        }
        if self
            .telegram_admin_bootstrap_secret
            .as_deref()
            .is_none_or(str::is_empty)
        {
            return Err(ConfigError::Missing("TELEGRAM_ADMIN_BOOTSTRAP_SECRET"));
        }

        self.validate_runtime_settings()?;
        self.validate_path_setting(
            "TELEGRAM_SESSION_PATH",
            Path::new(
                self.telegram_session_path
                    .as_deref()
                    .ok_or(ConfigError::Missing("TELEGRAM_SESSION_PATH"))?,
            ),
            false,
        )?;
        self.validate_path_setting("TELEGRAM_METADATA_PATH", &self.metadata_path(), false)?;
        self.validate_path_setting("TELEGRAM_DATA_DIR", &self.data_dir(), true)?;
        if !self.admin_bind_addr()?.ip().is_loopback() {
            return Err(ConfigError::Invalid("TELEGRAM_ADMIN_BIND_ADDR"));
        }

        if let Some(proxy_url) = &self.telegram_proxy_url {
            if !proxy_url.is_empty()
                && !proxy_url.starts_with("socks5://")
                && !proxy_url.starts_with("http://")
                && !proxy_url.starts_with("https://")
            {
                return Err(ConfigError::Invalid("TELEGRAM_PROXY_URL"));
            }
            if self.proxy_mode() == "direct" && !proxy_url.is_empty() {
                return Err(ConfigError::Invalid("TELEGRAM_PROXY_MODE"));
            }
        }

        Ok(())
    }

    fn validate_runtime_settings(&self) -> Result<(), ConfigError> {
        self.chunk_size()?;
        self.connection_timeout_secs()?;
        self.request_timeout_secs()?;
        self.transfer_timeout_secs()?;
        self.retry_count()?;
        self.retry_backoff_ms()?;
        self.respect_flood_wait()?;

        let proxy_mode = self.proxy_mode();
        match proxy_mode.as_str() {
            "auto" | "direct" | "socks5" => Ok(()),
            _ => Err(ConfigError::Invalid("TELEGRAM_PROXY_MODE")),
        }
    }

    fn validate_path_setting(
        &self,
        field: &'static str,
        path: &Path,
        directory: bool,
    ) -> Result<(), ConfigError> {
        if has_parent_dir_component(path) {
            return Err(ConfigError::PathUnsafe {
                field,
                path: path.display().to_string(),
                reason: "parent directory traversal is not allowed",
            });
        }

        if let Ok(metadata) = fs::symlink_metadata(path) {
            if metadata.file_type().is_symlink() {
                return Err(ConfigError::PathUnsafe {
                    field,
                    path: path.display().to_string(),
                    reason: "symbolic links are not allowed",
                });
            }
            if directory && !metadata.is_dir() {
                return Err(ConfigError::PathUnsafe {
                    field,
                    path: path.display().to_string(),
                    reason: "expected a directory path",
                });
            }
            if !directory && !metadata.is_file() {
                return Err(ConfigError::PathUnsafe {
                    field,
                    path: path.display().to_string(),
                    reason: "expected a file path",
                });
            }
            self.validate_permissions(field, path, &metadata, directory)?;
        } else if let Some(parent) = path.parent()
            && let Ok(parent_metadata) = fs::symlink_metadata(parent)
            && parent_metadata.file_type().is_symlink()
        {
            return Err(ConfigError::PathUnsafe {
                field,
                path: parent.display().to_string(),
                reason: "parent directory is a symbolic link",
            });
        }

        Ok(())
    }

    #[allow(unused_variables)]
    fn validate_permissions(
        &self,
        field: &'static str,
        path: &Path,
        metadata: &fs::Metadata,
        directory: bool,
    ) -> Result<(), ConfigError> {
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = metadata.permissions().mode();
            let unsafe_bits = if directory { 0o002 } else { 0o077 };
            if mode & unsafe_bits != 0 {
                return Err(ConfigError::UnsafePermissions {
                    field,
                    path: path.display().to_string(),
                    reason: "group or other access is too permissive",
                });
            }
        }
        Ok(())
    }
}

fn read(name: &'static str) -> Option<String> {
    env::var(name).ok().filter(|value| !value.trim().is_empty())
}

fn parse_u64(
    field: &'static str,
    value: Option<&str>,
    min: u64,
    max: u64,
    default: u64,
) -> Result<u64, ConfigError> {
    match value {
        Some(value) => {
            let parsed = value.parse::<u64>().map_err(|_| ConfigError::Parse {
                field,
                value: value.to_string(),
            })?;
            if parsed < min || parsed > max {
                return Err(ConfigError::Invalid(field));
            }
            Ok(parsed)
        }
        None => Ok(default),
    }
}

fn parse_u32(
    field: &'static str,
    value: Option<&str>,
    min: u32,
    max: u32,
    default: u32,
) -> Result<u32, ConfigError> {
    match value {
        Some(value) => {
            let parsed = value.parse::<u32>().map_err(|_| ConfigError::Parse {
                field,
                value: value.to_string(),
            })?;
            if parsed < min || parsed > max {
                return Err(ConfigError::Invalid(field));
            }
            Ok(parsed)
        }
        None => Ok(default),
    }
}

fn parse_bool(
    field: &'static str,
    value: Option<&str>,
    default: bool,
) -> Result<bool, ConfigError> {
    match value {
        Some(value) => match value {
            "true" | "1" | "yes" | "on" => Ok(true),
            "false" | "0" | "no" | "off" => Ok(false),
            _ => Err(ConfigError::Parse {
                field,
                value: value.to_string(),
            }),
        },
        None => Ok(default),
    }
}

fn has_parent_dir_component(path: &Path) -> bool {
    path.components()
        .any(|component| matches!(component, Component::ParentDir))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validate_requires_core_credentials() {
        let err = AppConfig::default()
            .validate()
            .expect_err("missing config should fail");
        assert_eq!(err, ConfigError::Missing("TELEGRAM_API_ID"));
    }

    #[test]
    fn effective_values_use_defaults() {
        let config = AppConfig::default();
        assert_eq!(config.metadata_path(), PathBuf::from(DEFAULT_METADATA_PATH));
        assert_eq!(config.data_dir(), PathBuf::from(DEFAULT_DATA_DIR));
        assert_eq!(config.chunk_size().expect("chunk"), 1_048_576);
        assert_eq!(config.retry_count().expect("retry"), 5);
        assert!(config.respect_flood_wait().expect("respect"));
    }

    #[test]
    fn parses_boolean_and_numeric_settings() {
        let config = AppConfig {
            telegram_chunk_size: Some("2048".to_string()),
            telegram_connection_timeout_secs: Some("15".to_string()),
            telegram_request_timeout_secs: Some("16".to_string()),
            telegram_transfer_timeout_secs: Some("17".to_string()),
            telegram_retry_count: Some("4".to_string()),
            telegram_retry_backoff_ms: Some("250".to_string()),
            telegram_respect_flood_wait: Some("false".to_string()),
            ..AppConfig::default()
        };
        assert_eq!(config.chunk_size().expect("chunk"), 2048);
        assert_eq!(config.connection_timeout_secs().expect("connect"), 15);
        assert_eq!(config.request_timeout_secs().expect("request"), 16);
        assert_eq!(config.transfer_timeout_secs().expect("transfer"), 17);
        assert_eq!(config.retry_count().expect("retry"), 4);
        assert_eq!(config.retry_backoff_ms().expect("backoff"), 250);
        assert!(!config.respect_flood_wait().expect("respect"));
    }
}
