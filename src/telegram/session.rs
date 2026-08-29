use grammers_session::storages::SqliteSession;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use thiserror::Error;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SessionStatus {
    Missing,
    Reusable,
    Reopened,
    Invalid,
    LoggedOut,
}

#[derive(Debug, Error)]
pub enum SessionError {
    #[error("failed to open Telegram session: {0}")]
    Open(String),
    #[error("failed to persist Telegram session: {0}")]
    Persist(String),
}

pub struct TelegramSession {
    path: PathBuf,
    storage: Arc<SqliteSession>,
}

impl TelegramSession {
    pub async fn open(path: impl AsRef<Path>) -> Result<Self, SessionError> {
        let path = path.as_ref().to_path_buf();
        if let Some(parent) = path.parent() {
            tokio::fs::create_dir_all(parent).await.ok();
        }
        let storage = SqliteSession::open(&path)
            .await
            .map_err(|error| SessionError::Open(error.to_string()))?;
        Ok(Self {
            path,
            storage: Arc::new(storage),
        })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn storage(&self) -> Arc<SqliteSession> {
        Arc::clone(&self.storage)
    }

    pub async fn reopen(path: impl AsRef<Path>) -> Result<Self, SessionError> {
        Self::open(path).await
    }

    pub fn status(&self) -> SessionStatus {
        if self.path.exists() {
            SessionStatus::Reusable
        } else {
            SessionStatus::Missing
        }
    }
}
