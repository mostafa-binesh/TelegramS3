use rusqlite::Connection;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::str::FromStr;
use std::sync::Mutex;
use thiserror::Error;

const SCHEMA_VERSION: u32 = 11;

#[derive(Debug, Error)]
pub enum MetadataError {
    #[error("sqlite error: {0}")]
    Sqlite(#[from] rusqlite::Error),
    #[error("serialization error: {0}")]
    Serde(#[from] serde_json::Error),
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("unsupported schema version {0}")]
    UnsupportedSchemaVersion(u32),
    #[error("manifest not found: {0}")]
    ManifestNotFound(String),
    #[error("journal entry not found: {0}")]
    JournalNotFound(String),
    #[error("multipart session not found: {0}")]
    MultipartSessionNotFound(String),
    #[error("invalid manifest: {0}")]
    InvalidManifest(String),
    #[error("precondition failed: {0}")]
    PreconditionFailed(String),
    #[error("invalid operation kind: {0}")]
    InvalidOperationKind(String),
    #[error("bucket not found: {0}")]
    BucketNotFound(String),
    #[error("bucket already exists: {0}")]
    BucketAlreadyExists(String),
    #[error("bucket not empty: {0}")]
    BucketNotEmpty(String),
    #[error("folder not empty: {0}")]
    FolderNotEmpty(String),
    #[error("a connection removal is already in progress")]
    ConnectionRemovalInProgress,
    #[error("metadata state is poisoned")]
    Poisoned,
    #[error("time formatting error: {0}")]
    TimeFormat(#[from] time::error::Format),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OperationKind {
    Put,
    Delete,
    Rebuild,
    Repair,
    GarbageCollect,
}

impl OperationKind {
    fn as_str(self) -> &'static str {
        match self {
            OperationKind::Put => "put",
            OperationKind::Delete => "delete",
            OperationKind::Rebuild => "rebuild",
            OperationKind::Repair => "repair",
            OperationKind::GarbageCollect => "garbage_collect",
        }
    }
}

impl FromStr for OperationKind {
    type Err = MetadataError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "put" => Ok(OperationKind::Put),
            "delete" => Ok(OperationKind::Delete),
            "rebuild" => Ok(OperationKind::Rebuild),
            "repair" => Ok(OperationKind::Repair),
            "garbage_collect" => Ok(OperationKind::GarbageCollect),
            _ => Err(MetadataError::InvalidOperationKind(value.to_string())),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MetadataStatus {
    pub path: Option<PathBuf>,
    pub schema_version: u32,
    pub buckets: u64,
    pub committed_objects: u64,
    pub active_objects: u64,
    pub staged_objects: u64,
    pub recovery_markers: u64,
}

pub struct MetadataStore {
    path: Option<PathBuf>,
    connection: Mutex<Connection>,
}

impl MetadataStore {
    pub fn path(&self) -> Option<&Path> {
        self.path.as_deref()
    }

    pub(crate) fn with_connection<T>(
        &self,
        f: impl FnOnce(&mut Connection) -> Result<T, MetadataError>,
    ) -> Result<T, MetadataError> {
        let mut connection = self
            .connection
            .lock()
            .map_err(|_| MetadataError::Poisoned)?;
        f(&mut connection)
    }
}

mod auth;
mod buckets;
mod connection_removal;
mod manifests;
mod multipart;
mod recovery;
mod rows;
mod schema;
mod settings;
mod shares;

pub use self::auth::{DbSession, DbUser};
pub use self::buckets::BucketRecord;
pub use self::connection_removal::ConnectionRemovalJob;
pub use self::manifests::{JournalEntry, TombstonedManifestRecord};
pub use self::recovery::{RebuildReport, VerifyReport};
pub use self::settings::{RecoveryAck, RecoveryAcknowledgements, TelegramBootstrapSettings};
pub use self::shares::ShareLinkRecord;
