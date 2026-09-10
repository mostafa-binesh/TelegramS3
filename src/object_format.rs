mod reception;
mod workflow;
use crate::config::AppConfig;
use crate::manifest::{ChunkRef, CommitState, ObjectChecksum, ObjectManifest, TelegramLocation};
use crate::metadata::{
    BucketRecord, JournalEntry, MetadataError, MetadataStatus, MetadataStore, OperationKind,
};
use crate::multipart::{MultipartCompletionPlan, MultipartPart, MultipartSession, MultipartState};
use crate::telegram::{TelegramConnectionState, TelegramTransportManager};
use bytes::Bytes;
use futures::StreamExt;
use grammers_client::media::Media;
use grammers_client::message::InputMessage;
use ring::aead::{Aad, CHACHA20_POLY1305, LessSafeKey, Nonce, UnboundKey};
use s3s::{Body, dto::StreamingBlob};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, HashMap};
use std::fs::{self, File};
use std::io::{self, Read, Write};
use std::ops::Range;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, RwLock};
use thiserror::Error;
use time::{Duration, OffsetDateTime};
use tokio::fs as async_fs;
use tokio::io::{AsyncSeekExt, AsyncWriteExt};
use tracing::warn;
use uuid::Uuid;

pub(crate) type RecoverySnapshot = (Option<i64>, Vec<RecoveryIssue>, Option<String>);

const CHECKSUM_ALGORITHM: &str = "sha256";
const ENCRYPTION_FORMAT: &str = "chacha20poly1305-v1";
pub const GARBAGE_COLLECTION_RETENTION_SECONDS: i64 = 7 * 24 * 60 * 60;
const MANIFEST_FILE_NAME: &str = "manifest.json";
const STAGING_ROOT: &str = "staging";
const MANIFEST_ROOT: &str = "manifests";
const CHUNK_ROOT: &str = "chunks";
const QUARANTINE_ROOT: &str = "quarantine";
const MULTIPART_ROOT: &str = "multipart";
const MOCK_TELEGRAM_ROOT: &str = "mock-telegram";
const CLEANUP_EVIDENCE_ROOT: &str = "cleanup-evidence";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChunkPlan {
    pub chunk_size: u64,
    pub content_length: u64,
    pub chunks: Vec<PlannedChunk>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlannedChunk {
    pub order: u32,
    pub offset: u64,
    pub size: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReadPlan {
    pub object_id: Uuid,
    pub requested_range: Range<u64>,
    pub chunks: Vec<ReadSpan>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReadSpan {
    pub order: u32,
    pub offset_within_chunk: u64,
    pub length: u64,
    pub path: PathBuf,
    pub checksum: String,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct ReconciliationReport {
    pub staged_objects: u64,
    pub committed_objects: u64,
    pub recovery_required_objects: u64,
    pub orphaned_chunks: u64,
    pub repaired_objects: u64,
    pub quarantined_objects: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GarbageCollectionReport {
    pub dry_run: bool,
    pub eligible_objects: u64,
    pub manifests_removed: u64,
    pub chunk_directories_removed: u64,
    pub quarantine_entries_removed: u64,
    pub bytes_removed: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ObjectFormatStatus {
    pub data_dir: PathBuf,
    pub chunk_size: u64,
    pub committed_objects: u64,
    pub staged_objects: u64,
    pub recovery_required_objects: u64,
    pub orphaned_chunks: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub struct RecoveryIssue {
    pub object_id: Option<Uuid>,
    pub bucket: Option<String>,
    pub key: Option<String>,
    pub path: Option<String>,
    pub commit_state: Option<CommitState>,
    pub kind: String,
    pub summary: String,
    pub details: Vec<String>,
}

impl RecoveryIssue {
    /// Stable identity for an issue across repeated scans, used to remember
    /// operator acknowledgements. Only the fields that identify *what* is broken
    /// take part; `summary` and `details` are deliberately excluded so that
    /// rewording a scan message cannot silently drop an acknowledgement.
    pub fn fingerprint(&self) -> String {
        let mut hasher = Sha256::new();
        for part in [
            Some(self.kind.as_str()),
            self.object_id.as_ref().map(|_| "object"),
            self.bucket.as_deref(),
            self.key.as_deref(),
            self.path.as_deref(),
        ] {
            // Length-prefix each field so ("ab", "c") cannot collide with ("a", "bc").
            match part {
                Some(value) => {
                    hasher.update(value.len().to_le_bytes());
                    hasher.update(value.as_bytes());
                }
                None => hasher.update(usize::MAX.to_le_bytes()),
            }
        }
        if let Some(object_id) = self.object_id {
            hasher.update(object_id.as_bytes());
        }
        let digest = hasher.finalize();
        digest[..8]
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StagedObject {
    pub operation_id: Uuid,
    pub object_id: Uuid,
    pub manifest: ObjectManifest,
    pub chunk_plan: ChunkPlan,
}

#[derive(Debug, Clone)]
pub(crate) struct ObjectEncryption {
    key: [u8; 32],
    key_id: String,
}

impl ObjectEncryption {
    fn from_master_key(master_key: &str) -> Self {
        let mut hasher = Sha256::new();
        hasher.update(master_key.as_bytes());
        let key_bytes = hasher.finalize();
        let mut key = [0_u8; 32];
        key.copy_from_slice(&key_bytes);
        let mut key_id_hasher = Sha256::new();
        key_id_hasher.update(b"telegram-s3-encryption-key");
        key_id_hasher.update(master_key.as_bytes());
        let key_id = hex::encode(key_id_hasher.finalize());
        Self { key, key_id }
    }

    fn chunk_key(&self, object_id: Uuid) -> [u8; 32] {
        let mut hasher = Sha256::new();
        hasher.update(self.key);
        hasher.update(object_id.as_bytes());
        let digest = hasher.finalize();
        let mut key = [0_u8; 32];
        key.copy_from_slice(&digest);
        key
    }

    fn nonce(&self, object_id: Uuid, order: u32) -> [u8; 12] {
        let mut hasher = Sha256::new();
        hasher.update(object_id.as_bytes());
        hasher.update(order.to_le_bytes());
        let digest = hasher.finalize();
        let mut nonce = [0_u8; 12];
        nonce.copy_from_slice(&digest[..12]);
        nonce
    }

    fn encrypt_chunk(
        &self,
        object_id: Uuid,
        order: u32,
        plaintext: &[u8],
    ) -> Result<Vec<u8>, ObjectFormatError> {
        let unbound = UnboundKey::new(&CHACHA20_POLY1305, &self.chunk_key(object_id))
            .map_err(|_| ObjectFormatError::InvalidChecksum("invalid key".to_string()))?;
        let cipher = LessSafeKey::new(unbound);
        let nonce = Nonce::try_assume_unique_for_key(&self.nonce(object_id, order))
            .map_err(|_| ObjectFormatError::InvalidChecksum("invalid nonce".to_string()))?;
        let mut in_out = plaintext.to_vec();
        cipher
            .seal_in_place_append_tag(nonce, Aad::from(object_id.as_bytes()), &mut in_out)
            .map_err(|_| ObjectFormatError::InvalidChecksum("encryption failed".to_string()))?;
        Ok(in_out)
    }

    pub(crate) fn decrypt_chunk(
        &self,
        object_id: Uuid,
        order: u32,
        ciphertext: &[u8],
    ) -> Result<Vec<u8>, ObjectFormatError> {
        let unbound = UnboundKey::new(&CHACHA20_POLY1305, &self.chunk_key(object_id))
            .map_err(|_| ObjectFormatError::InvalidChecksum("invalid key".to_string()))?;
        let cipher = LessSafeKey::new(unbound);
        let nonce = Nonce::try_assume_unique_for_key(&self.nonce(object_id, order))
            .map_err(|_| ObjectFormatError::InvalidChecksum("invalid nonce".to_string()))?;
        let mut in_out = ciphertext.to_vec();
        let plaintext = cipher
            .open_in_place(nonce, Aad::from(object_id.as_bytes()), &mut in_out)
            .map_err(|_| ObjectFormatError::ChecksumMismatch {
                scope: format!("chunk {}", order),
                expected: "encrypted payload".to_string(),
                actual: "decryption failed".to_string(),
            })?;
        Ok(plaintext.to_vec())
    }
}

struct ManifestBuildArgs {
    object_id: Uuid,
    bucket: String,
    key: String,
    content_type: String,
    commit_state: CommitState,
    chunks: Vec<ChunkRef>,
    whole_checksum: String,
}

#[derive(Debug, Error)]
pub enum ObjectFormatError {
    #[error("{0}")]
    Config(#[from] crate::config::ConfigError),
    #[error("{0}")]
    Metadata(#[from] MetadataError),
    #[error("{0}")]
    Telegram(#[from] crate::telegram::TelegramTransportError),
    #[error("{0}")]
    Serde(#[from] serde_json::Error),
    #[error("io error: {0}")]
    Io(#[from] io::Error),
    #[error("invalid upload plan: {0}")]
    InvalidPlan(String),
    #[error("invalid read plan: {0}")]
    InvalidRead(String),
    #[error("checksum mismatch for {scope}: expected {expected}, got {actual}")]
    ChecksumMismatch {
        scope: String,
        expected: String,
        actual: String,
    },
    #[error("missing chunk {order} for object {object_id}")]
    MissingChunk { object_id: Uuid, order: u32 },
    #[error("bootstrap blocked: {0}")]
    RecoveryRequired(String),
    #[error("invalid checksum: {0}")]
    InvalidChecksum(String),
    #[error("resumable reception not found")]
    ReceptionNotFound,
    #[error("resumable reception offset mismatch; server received {received}")]
    ReceptionOffsetMismatch { received: u64 },
    #[error("resumable reception chunk exceeds the configured chunk size")]
    ReceptionChunkTooLarge,
    #[error("resumable reception chunks must fill the chunk size unless final")]
    ReceptionChunkSizeMismatch,
    #[error("resumable reception is not complete")]
    ReceptionNotComplete,
}

#[derive(Clone)]
pub struct ObjectFormatService {
    metadata: Arc<MetadataStore>,
    transport_manager: std::sync::Arc<TelegramTransportManager>,
    data_dir: PathBuf,
    chunk_size: u64,
    storage_chat_id: Arc<RwLock<String>>,
    worker_runtime: Arc<WorkerRuntime>,
    read_pins: Arc<Mutex<HashMap<Uuid, u64>>>,
    receptions: Arc<Mutex<HashMap<Uuid, Arc<tokio::sync::Mutex<reception::ReceptionState>>>>>,
    recovery_snapshot: Arc<RwLock<RecoverySnapshot>>,
    staging_budget: u64,
    encryption: ObjectEncryption,
}

#[derive(Default)]
struct WorkerRuntime {
    started: std::sync::atomic::AtomicBool,
    shutdown: Mutex<Option<tokio::sync::watch::Sender<bool>>>,
    handles: Mutex<Vec<tokio::task::JoinHandle<()>>>,
}

struct ReadPinGuard {
    object_id: Uuid,
    pins: Arc<Mutex<HashMap<Uuid, u64>>>,
}

impl Drop for ReadPinGuard {
    fn drop(&mut self) {
        if let Ok(mut pins) = self.pins.lock()
            && let Some(count) = pins.get_mut(&self.object_id)
        {
            *count = count.saturating_sub(1);
            if *count == 0 {
                pins.remove(&self.object_id);
            }
        }
    }
}

struct ReceivingGuard {
    metadata: Arc<MetadataStore>,
    job_id: String,
    path: PathBuf,
    armed: bool,
}

impl ReceivingGuard {
    fn disarm(&mut self) {
        self.armed = false;
    }
}

impl Drop for ReceivingGuard {
    fn drop(&mut self) {
        if !self.armed {
            return;
        }
        let files_removed = match fs::remove_dir_all(&self.path) {
            Ok(()) => true,
            Err(error) if error.kind() == io::ErrorKind::NotFound => true,
            Err(_) => false,
        };
        let _ = self.metadata.fail_reception(
            &self.job_id,
            files_removed,
            "Upload reception failed before durable acceptance; resend the source file",
        );
    }
}

impl ObjectFormatService {
    pub async fn open(config: &AppConfig) -> Result<Self, ObjectFormatError> {
        let transport_manager = TelegramTransportManager::open(config.clone()).await?;
        Self::open_with_transport_manager(config, transport_manager).await
    }

    pub async fn open_with_transport_manager(
        config: &AppConfig,
        transport_manager: std::sync::Arc<TelegramTransportManager>,
    ) -> Result<Self, ObjectFormatError> {
        config.validate()?;
        let metadata = MetadataStore::open(config.metadata_path())?;
        let master_key = config
            .telegram_s3_master_key
            .clone()
            .ok_or_else(|| ObjectFormatError::InvalidPlan("missing master key".to_string()))?;
        let storage_chat_id = config
            .resolve_telegram_bootstrap(&metadata)
            .ok()
            .map(|bootstrap| bootstrap.telegram_storage_chat_id)
            .unwrap_or_default();
        let mut service = Self::new(
            metadata,
            transport_manager,
            config.data_dir(),
            config.chunk_size()?,
            storage_chat_id,
            ObjectEncryption::from_master_key(&master_key),
        )?;
        service.staging_budget = config.staging_budget()?;
        let _ = service.refresh_recovery_snapshot().await;
        Ok(service)
    }

    pub(crate) fn new(
        metadata: MetadataStore,
        transport_manager: std::sync::Arc<TelegramTransportManager>,
        data_dir: impl AsRef<Path>,
        chunk_size: u64,
        storage_chat_id: String,
        encryption: ObjectEncryption,
    ) -> Result<Self, ObjectFormatError> {
        let data_dir = data_dir.as_ref().to_path_buf();
        fs::create_dir_all(data_dir.join(STAGING_ROOT))?;
        fs::create_dir_all(data_dir.join(MANIFEST_ROOT))?;
        fs::create_dir_all(data_dir.join(CHUNK_ROOT))?;
        fs::create_dir_all(data_dir.join(QUARANTINE_ROOT))?;
        fs::create_dir_all(data_dir.join(MULTIPART_ROOT))?;
        fs::create_dir_all(data_dir.join(CLEANUP_EVIDENCE_ROOT))?;
        Ok(Self {
            metadata: Arc::new(metadata),
            transport_manager,
            data_dir,
            chunk_size,
            storage_chat_id: Arc::new(RwLock::new(storage_chat_id)),
            worker_runtime: Arc::new(WorkerRuntime::default()),
            read_pins: Arc::new(Mutex::new(HashMap::new())),
            receptions: Arc::new(Mutex::new(HashMap::new())),
            recovery_snapshot: Arc::new(RwLock::new((
                None,
                Vec::new(),
                Some("Recovery scan pending".into()),
            ))),
            staging_budget: 10 * 1024 * 1024 * 1024,
            encryption,
        })
    }

    pub fn metadata_status(&self) -> Result<MetadataStatus, ObjectFormatError> {
        Ok(self.metadata.status()?)
    }

    pub fn durable_metrics(&self) -> Result<crate::durable::DurableMetrics, ObjectFormatError> {
        Ok(self.metadata.durable_metrics()?)
    }

    pub fn staging_budget(&self) -> u64 {
        self.staging_budget
    }

    pub fn chunk_size(&self) -> u64 {
        self.chunk_size
    }

    /// Reach the shared SQLite store (single writer) for operator/auth tables.
    pub fn metadata_store(&self) -> &MetadataStore {
        &self.metadata
    }

    pub fn set_storage_chat_id(&self, storage_chat_id: String) {
        *self.storage_chat_id.write().expect("storage chat id lock") = storage_chat_id;
    }

    fn storage_chat_id(&self) -> Result<String, ObjectFormatError> {
        let storage_chat_id = self
            .storage_chat_id
            .read()
            .expect("storage chat id lock")
            .clone();
        if storage_chat_id.is_empty() {
            return Err(ObjectFormatError::InvalidPlan(
                "telegram bootstrap settings are not configured".to_string(),
            ));
        }
        Ok(storage_chat_id)
    }

    pub fn list_manifests(&self) -> Result<Vec<ObjectManifest>, ObjectFormatError> {
        Ok(self.metadata.list_manifests()?)
    }

    pub fn create_bucket(&self, bucket: &str) -> Result<BucketRecord, ObjectFormatError> {
        self.ensure_connection_not_removing()?;
        Ok(self.metadata.create_bucket(BucketRecord {
            name: bucket.to_string(),
            created_at: OffsetDateTime::now_utc(),
            deleted_at: None,
            versioning_enabled: false,
            object_locking_enabled: false,
        })?)
    }

    pub fn get_bucket(&self, bucket: &str) -> Result<Option<BucketRecord>, ObjectFormatError> {
        Ok(self.metadata.get_bucket(bucket)?)
    }

    pub fn list_buckets(&self) -> Result<Vec<BucketRecord>, ObjectFormatError> {
        Ok(self.metadata.list_buckets()?)
    }

    pub fn delete_bucket(&self, bucket: &str) -> Result<(), ObjectFormatError> {
        Ok(self.metadata.delete_bucket(bucket)?)
    }

    pub fn bucket_exists(&self, bucket: &str) -> Result<bool, ObjectFormatError> {
        Ok(self.metadata.bucket_exists(bucket)?)
    }

    pub fn get_active_manifest(
        &self,
        bucket: &str,
        key: &str,
    ) -> Result<Option<ObjectManifest>, ObjectFormatError> {
        Ok(self.metadata.get_active_manifest(bucket, key)?)
    }

    pub fn list_bucket_manifests(
        &self,
        bucket: &str,
        prefix: Option<&str>,
    ) -> Result<Vec<ObjectManifest>, ObjectFormatError> {
        Ok(self.metadata.list_bucket_manifests(bucket, prefix)?)
    }

    pub fn delete_object(
        &self,
        bucket: &str,
        key: &str,
        if_match: Option<&s3s::dto::ETagCondition>,
        if_match_last_modified_time: Option<&s3s::dto::Timestamp>,
        if_match_size: Option<i64>,
    ) -> Result<Option<ObjectManifest>, ObjectFormatError> {
        Ok(self.metadata.delete_active_key(
            bucket,
            key,
            "deleted via S3",
            if_match,
            if_match_last_modified_time,
            if_match_size,
        )?)
    }

    pub fn tombstone_manifest(
        &self,
        object_id: Uuid,
        reason: &str,
    ) -> Result<ObjectManifest, ObjectFormatError> {
        Ok(self.metadata.tombstone_manifest(object_id, reason)?)
    }

    pub fn tombstone_manifest_with_conditionals(
        &self,
        object_id: Uuid,
        reason: &str,
        if_match: Option<&s3s::dto::ETagCondition>,
        if_match_last_modified_time: Option<&s3s::dto::Timestamp>,
        if_match_size: Option<i64>,
    ) -> Result<ObjectManifest, ObjectFormatError> {
        Ok(self.metadata.tombstone_manifest_with_conditionals(
            object_id,
            reason,
            if_match,
            if_match_last_modified_time,
            if_match_size,
        )?)
    }

    pub fn initiate_multipart_upload(
        &self,
        bucket: &str,
        key: &str,
        content_type: &str,
        checksum_algorithm: Option<&str>,
    ) -> Result<MultipartSession, ObjectFormatError> {
        self.ensure_connection_not_removing()?;
        if !self.bucket_exists(bucket)? {
            return Err(ObjectFormatError::InvalidPlan(format!(
                "bucket does not exist: {bucket}"
            )));
        }
        let upload_id = Uuid::new_v4();
        let session = MultipartSession {
            upload_id,
            bucket: bucket.to_string(),
            key: key.to_string(),
            version_id: Some(upload_id.to_string()),
            content_type: content_type.to_string(),
            checksum_algorithm: checksum_algorithm.unwrap_or(CHECKSUM_ALGORITHM).to_string(),
            state: MultipartState::Initiated,
            created_at: OffsetDateTime::now_utc(),
            updated_at: OffsetDateTime::now_utc(),
        };
        self.metadata.create_multipart_session(session.clone())?;
        fs::create_dir_all(self.multipart_dir(upload_id))?;
        Ok(session)
    }

    pub async fn upload_multipart_part(
        &self,
        upload_id: Uuid,
        part_number: u32,
        body: Option<StreamingBlob>,
        checksum: Option<&str>,
    ) -> Result<MultipartPart, ObjectFormatError> {
        self.ensure_connection_not_removing()?;
        if part_number == 0 {
            return Err(ObjectFormatError::InvalidPlan(
                "part number must be at least 1".to_string(),
            ));
        }
        let session = self
            .metadata
            .get_multipart_session(upload_id)?
            .ok_or_else(|| {
                ObjectFormatError::InvalidPlan(format!("upload not found: {upload_id}"))
            })?;
        if matches!(
            session.state,
            MultipartState::Aborted | MultipartState::Completed | MultipartState::Quarantined
        ) {
            return Err(ObjectFormatError::InvalidPlan(format!(
                "multipart upload is not active: {upload_id}"
            )));
        }

        let job = self
            .enqueue_with_part(
                &session.bucket,
                &session.key,
                &session.content_type,
                body,
                Some((upload_id, part_number, checksum.map(str::to_string))),
                None,
            )
            .await?;
        self.wait_transfer(&job.id).await?;
        self.metadata
            .get_multipart_part(upload_id, part_number)?
            .ok_or_else(|| ObjectFormatError::InvalidPlan("completed part missing".into()))
    }

    pub fn list_multipart_uploads(
        &self,
        bucket: &str,
        prefix: Option<&str>,
    ) -> Result<Vec<MultipartSession>, ObjectFormatError> {
        Ok(self
            .metadata
            .list_multipart_sessions(Some(bucket), prefix)?)
    }

    pub fn get_multipart_session(
        &self,
        upload_id: Uuid,
    ) -> Result<Option<MultipartSession>, ObjectFormatError> {
        Ok(self.metadata.get_multipart_session(upload_id)?)
    }

    pub fn list_multipart_parts(
        &self,
        upload_id: Uuid,
    ) -> Result<Vec<MultipartPart>, ObjectFormatError> {
        Ok(self.metadata.list_multipart_parts(upload_id)?)
    }

    pub async fn complete_multipart_upload(
        &self,
        plan: MultipartCompletionPlan,
    ) -> Result<ObjectManifest, ObjectFormatError> {
        let session = self
            .metadata
            .get_multipart_session(plan.upload_id)?
            .ok_or_else(|| {
                ObjectFormatError::InvalidPlan(format!("upload not found: {}", plan.upload_id))
            })?;
        if session.bucket != plan.bucket || session.key != plan.key {
            return Err(ObjectFormatError::InvalidPlan(
                "multipart completion target mismatch".to_string(),
            ));
        }
        if session.state == MultipartState::Completed {
            return self
                .metadata
                .get_active_manifest(&plan.bucket, &plan.key)?
                .ok_or_else(|| {
                    ObjectFormatError::InvalidPlan(
                        "completed multipart object is not visible".into(),
                    )
                });
        }
        if matches!(
            session.state,
            MultipartState::Aborted
                | MultipartState::Quarantined
                | MultipartState::RecoveryRequired
        ) {
            return Err(ObjectFormatError::InvalidPlan(
                "multipart upload is not completable".into(),
            ));
        }

        let stored_parts = self.metadata.list_multipart_parts(plan.upload_id)?;
        let stored_parts_by_number: HashMap<u32, MultipartPart> = stored_parts
            .into_iter()
            .map(|part| (part.part_number, part))
            .collect();

        if plan.parts.is_empty() {
            return Err(ObjectFormatError::InvalidPlan(
                "multipart completion requires at least one part".to_string(),
            ));
        }

        let mut sources = Vec::new();
        for part_plan in &plan.parts {
            let stored = stored_parts_by_number
                .get(&part_plan.part_number)
                .ok_or_else(|| ObjectFormatError::InvalidPlan("missing multipart part".into()))?;
            if stored.e_tag != part_plan.e_tag {
                return Err(ObjectFormatError::InvalidPlan(
                    "multipart ETag mismatch".into(),
                ));
            }
            let Some(manifest) = stored.manifest.clone() else {
                self.metadata.update_multipart_session_state(
                    plan.upload_id,
                    MultipartState::RecoveryRequired,
                )?;
                return Err(ObjectFormatError::InvalidPlan(
                    "legacy multipart framing requires recovery or source re-upload".into(),
                ));
            };
            sources.push(manifest);
        }
        let service = Arc::new(self.clone());
        let stream = futures::stream::iter(sources).flat_map(move |manifest| {
            let spans = Self::plan_read(&manifest, 0..manifest.content_length)
                .expect("validated part spans");
            Self::read_spans_to_stream(Arc::clone(&service), &manifest, spans.chunks)
        });
        let body: StreamingBlob = Body::http_body_unsync(http_body_util::StreamBody::new(
            stream.map(|chunk| chunk.map(hyper::body::Frame::data)),
        ))
        .into();
        let job = self
            .enqueue_with_part(
                &plan.bucket,
                &plan.key,
                &plan.content_type,
                Some(body),
                Some((plan.upload_id, 0, None)),
                None,
            )
            .await?;
        self.wait_transfer(&job.id).await
    }

    pub async fn abort_multipart_upload(&self, upload_id: Uuid) -> Result<(), ObjectFormatError> {
        self.metadata.abort_parts(upload_id)?;
        Ok(())
    }

    pub fn get_manifest_by_version(
        &self,
        bucket: &str,
        key: &str,
        version_id: &str,
    ) -> Result<Option<ObjectManifest>, ObjectFormatError> {
        let object_id = Uuid::parse_str(version_id)
            .map_err(|error| ObjectFormatError::InvalidPlan(error.to_string()))?;
        let manifest = self.metadata.get_manifest(object_id)?;
        Ok(manifest.filter(|manifest| manifest.bucket == bucket && manifest.key == key))
    }

    pub fn data_dir(&self) -> &Path {
        &self.data_dir
    }

    pub fn chunk_path(&self, object_id: Uuid, order: u32) -> PathBuf {
        self.chunk_dir(object_id).join(chunk_file_name(order))
    }

    pub fn manifest_file_path(&self, object_id: Uuid) -> PathBuf {
        self.manifest_path(object_id)
    }

    pub fn status(&self) -> Result<ObjectFormatStatus, ObjectFormatError> {
        let manifests = self.metadata.list_manifests()?;
        let committed_objects = manifests
            .iter()
            .filter(|manifest| manifest.commit_state == CommitState::Committed)
            .count() as u64;
        let staged_objects = manifests
            .iter()
            .filter(|manifest| manifest.commit_state == CommitState::Staging)
            .count() as u64;
        let recovery_required_objects = manifests
            .iter()
            .filter(|manifest| manifest.commit_state == CommitState::RecoveryRequired)
            .count() as u64;
        let orphaned_chunks = manifests
            .iter()
            .filter(|manifest| manifest.commit_state == CommitState::Orphaned)
            .count() as u64;

        Ok(ObjectFormatStatus {
            data_dir: self.data_dir.clone(),
            chunk_size: self.chunk_size,
            committed_objects,
            staged_objects,
            recovery_required_objects,
            orphaned_chunks,
        })
    }

    pub async fn bootstrap(&self) -> Result<ObjectFormatStatus, ObjectFormatError> {
        let transport_health = self.transport_manager.health().await;
        if !matches!(transport_health.state, TelegramConnectionState::Connected) {
            warn!(
                "object-format bootstrap deferred remote reconciliation because Telegram is unavailable: {}",
                transport_health.detail
            );
            return self.status();
        }
        match self.reconcile().await {
            Ok(report) => {
                if report.staged_objects > 0 || report.recovery_required_objects > 0 {
                    warn!(
                        "object-format bootstrap completed with recovery state: staged_objects={}, recovery_required_objects={}",
                        report.staged_objects, report.recovery_required_objects
                    );
                }
            }
            Err(error) => {
                warn!(
                    "object-format bootstrap skipped remote reconciliation after Telegram error: {error}"
                );
            }
        }
        self.status()
    }

    pub async fn recovery_issues(&self) -> Result<Vec<RecoveryIssue>, ObjectFormatError> {
        let manifests = self.metadata.list_manifests()?;
        let journal_entries = self.metadata.list_journal_entries()?;
        let journal_by_operation: HashMap<Uuid, JournalEntry> = journal_entries
            .into_iter()
            .map(|entry| (entry.operation_id, entry))
            .collect();
        let mut issues = Vec::new();

        for manifest in manifests.into_iter().take(100) {
            match manifest.commit_state {
                CommitState::Staging => {
                    let operation_id = journal_by_operation
                        .values()
                        .find(|entry| {
                            entry.object_id == manifest.object_id && entry.state == "staging"
                        })
                        .map(|entry| entry.operation_id);
                    issues.extend(self.inspect_staging_manifest(&manifest, operation_id)?);
                }
                CommitState::Committed | CommitState::RecoveryRequired => {
                    issues.extend(self.inspect_committed_manifest(&manifest).await?);
                }
                CommitState::Orphaned => {
                    issues.push(RecoveryIssue {
                        object_id: Some(manifest.object_id),
                        bucket: Some(manifest.bucket.clone()),
                        key: Some(manifest.key.clone()),
                        path: Some(format!("{}/{}", manifest.bucket, manifest.key)),
                        commit_state: Some(manifest.commit_state),
                        kind: "orphaned".to_string(),
                        summary: "orphaned object data".to_string(),
                        details: vec![
                            "object metadata is marked orphaned and needs reconciliation"
                                .to_string(),
                        ],
                    });
                }
                CommitState::Tombstoned => {}
            }
        }

        if let Ok(entries) = fs::read_dir(self.data_dir.join(STAGING_ROOT)) {
            for entry in entries.take(100) {
                let entry = entry?;
                if !entry.file_type()?.is_dir() {
                    continue;
                }
                let dir_name = entry.file_name().to_string_lossy().to_string();
                if Uuid::parse_str(&dir_name).is_ok() {
                    continue;
                }
                issues.push(RecoveryIssue {
                    object_id: None,
                    bucket: None,
                    key: None,
                    path: Some(entry.path().display().to_string()),
                    commit_state: None,
                    kind: "orphaned_staging_dir".to_string(),
                    summary: "orphaned staging directory".to_string(),
                    details: vec!["directory is not tied to an active upload".to_string()],
                });
            }
        }

        if let Ok(entries) = fs::read_dir(self.data_dir.join(MANIFEST_ROOT)) {
            for entry in entries.take(100) {
                let entry = entry?;
                if !entry.file_type()?.is_file() {
                    continue;
                }
                let file_name = entry.file_name().to_string_lossy().to_string();
                let stem = file_name.trim_end_matches(".json");
                if Uuid::parse_str(stem).is_ok() {
                    continue;
                }
                issues.push(RecoveryIssue {
                    object_id: None,
                    bucket: None,
                    key: None,
                    path: Some(entry.path().display().to_string()),
                    commit_state: None,
                    kind: "orphaned_manifest_file".to_string(),
                    summary: "orphaned manifest file".to_string(),
                    details: vec!["manifest file does not match a tracked object".to_string()],
                });
            }
        }

        if let Ok(entries) = fs::read_dir(self.data_dir.join(CHUNK_ROOT)) {
            for entry in entries.take(100) {
                let entry = entry?;
                if !entry.file_type()?.is_dir() {
                    continue;
                }
                let dir_name = entry.file_name().to_string_lossy().to_string();
                if Uuid::parse_str(&dir_name).is_ok() {
                    continue;
                }
                issues.push(RecoveryIssue {
                    object_id: None,
                    bucket: None,
                    key: None,
                    path: Some(entry.path().display().to_string()),
                    commit_state: None,
                    kind: "orphaned_chunk_dir".to_string(),
                    summary: "orphaned chunk directory".to_string(),
                    details: vec!["chunk directory is not tied to a tracked object".to_string()],
                });
            }
        }

        issues.sort_by(|a, b| {
            a.bucket
                .cmp(&b.bucket)
                .then(a.key.cmp(&b.key))
                .then(a.path.cmp(&b.path))
                .then(a.kind.cmp(&b.kind))
        });
        Ok(issues)
    }

    pub async fn refresh_recovery_snapshot(&self) -> Result<(), ObjectFormatError> {
        let checked_at = OffsetDateTime::now_utc().unix_timestamp();
        match self.recovery_issues().await {
            Ok(issues) => {
                if let Ok(mut snapshot) = self.recovery_snapshot.write() {
                    *snapshot = (Some(checked_at), issues, None);
                }
                Ok(())
            }
            Err(error) => {
                let message = error.to_string();
                let previous_issues = self
                    .recovery_snapshot
                    .read()
                    .ok()
                    .map(|snapshot| snapshot.1.clone())
                    .unwrap_or_default();
                if let Ok(mut snapshot) = self.recovery_snapshot.write() {
                    *snapshot = (Some(checked_at), previous_issues, Some(message.clone()));
                }
                Err(error)
            }
        }
    }

    pub fn clear_recovery_snapshot(&self) {
        if let Ok(mut snapshot) = self.recovery_snapshot.write() {
            *snapshot = (
                Some(OffsetDateTime::now_utc().unix_timestamp()),
                Vec::new(),
                None,
            );
        }
    }

    pub fn cached_recovery_snapshot(&self) -> Result<RecoverySnapshot, String> {
        let snapshot = self
            .recovery_snapshot
            .read()
            .map_err(|_| "recovery cache unavailable".to_string())?;
        Ok(snapshot.clone())
    }

    pub fn plan_upload(&self, content_length: u64) -> Result<ChunkPlan, ObjectFormatError> {
        Self::plan_chunks(content_length, self.chunk_size)
    }

    pub fn stage_bytes(
        &self,
        bucket: &str,
        key: &str,
        content_type: &str,
        bytes: &[u8],
    ) -> Result<StagedObject, ObjectFormatError> {
        let mut reader = io::Cursor::new(bytes);
        self.stage_reader(bucket, key, content_type, &mut reader)
    }

    pub fn stage_reader<R: Read>(
        &self,
        bucket: &str,
        key: &str,
        content_type: &str,
        reader: &mut R,
    ) -> Result<StagedObject, ObjectFormatError> {
        self.ensure_connection_not_removing()?;
        let object_id = Uuid::new_v4();
        let job_id = self.metadata.begin_transfer(object_id, bucket, key)?;
        let scratch_dir = self
            .data_dir
            .join(STAGING_ROOT)
            .join(format!("upload-{object_id}"));
        let mut receiving = ReceivingGuard {
            metadata: Arc::clone(&self.metadata),
            job_id: job_id.clone(),
            path: scratch_dir.clone(),
            armed: true,
        };
        fs::create_dir_all(&scratch_dir)?;

        let mut chunk_plan = ChunkPlan {
            chunk_size: self.chunk_size,
            content_length: 0,
            chunks: Vec::new(),
        };
        let mut chunk_refs = Vec::new();
        let mut whole_hasher = Sha256::new();
        let mut buffer = vec![0_u8; self.chunk_size as usize];
        let mut offset = 0_u64;

        loop {
            let read = reader.read(&mut buffer)?;
            if read == 0 {
                break;
            }
            let chunk_bytes = &buffer[..read];
            let chunk_checksum = sha256_hex(chunk_bytes);
            let chunk_path = scratch_dir.join(chunk_file_name(chunk_plan.chunks.len() as u32));
            self.metadata
                .reserve_staging(&job_id, read as u64 + 16, self.staging_budget)?;
            let mut chunk_file = File::create(&chunk_path)?;
            let encrypted =
                self.encrypt_chunk(object_id, chunk_plan.chunks.len() as u32, chunk_bytes)?;
            chunk_file.write_all(&encrypted)?;
            chunk_file.sync_all()?;

            whole_hasher.update(chunk_bytes);
            let order = chunk_plan.chunks.len() as u32;
            let size = read as u64;
            chunk_plan.chunks.push(PlannedChunk {
                order,
                offset,
                size,
            });
            chunk_refs.push(ChunkRef {
                order,
                offset,
                size,
                checksum: chunk_checksum,
                telegram_peer_id: self.storage_chat_id()?,
                telegram_message_id: i64::from(order) + 1,
                telegram_document_id: Some(format!("local:{object_id}:{order}")),
            });
            chunk_plan.content_length =
                chunk_plan.content_length.checked_add(size).ok_or_else(|| {
                    ObjectFormatError::InvalidPlan("content length overflow".to_string())
                })?;
            offset = offset
                .checked_add(size)
                .ok_or_else(|| ObjectFormatError::InvalidPlan("offset overflow".to_string()))?;
        }

        let whole_checksum = hex::encode(whole_hasher.finalize());
        let manifest = self.new_manifest(ManifestBuildArgs {
            object_id,
            bucket: bucket.to_string(),
            key: key.to_string(),
            content_type: content_type.to_string(),
            commit_state: CommitState::Staging,
            chunks: chunk_refs,
            whole_checksum,
        });

        let operation_id = self
            .metadata
            .stage_manifest(OperationKind::Put, manifest.clone())?;
        let staging_dir = self.staging_dir(operation_id);
        if staging_dir != scratch_dir {
            if staging_dir.exists() {
                fs::remove_dir_all(&staging_dir)?;
            }
            fs::rename(&scratch_dir, &staging_dir)?;
            receiving.path = staging_dir.clone();
        }
        let stage_manifest_path = staging_dir.join(MANIFEST_FILE_NAME);
        write_json_file(&stage_manifest_path, &manifest)?;
        verify_staged_chunks(&staging_dir, &manifest, &self.encryption)?;
        self.metadata
            .queue_transfer(&job_id, operation_id, manifest.chunks.len())?;
        receiving.disarm();
        Ok(StagedObject {
            operation_id,
            object_id,
            manifest,
            chunk_plan,
        })
    }

    pub async fn commit_staged_object(
        &self,
        staged: &StagedObject,
    ) -> Result<ObjectManifest, ObjectFormatError> {
        self.wait_transfer(&staged.object_id.to_string()).await
    }

    pub async fn put_bytes(
        &self,
        bucket: &str,
        key: &str,
        content_type: &str,
        bytes: &[u8],
    ) -> Result<ObjectManifest, ObjectFormatError> {
        let staged = self.stage_bytes(bucket, key, content_type, bytes)?;
        self.commit_staged_object(&staged).await
    }

    pub async fn put_stream(
        &self,
        bucket: &str,
        key: &str,
        content_type: &str,
        body: Option<StreamingBlob>,
        conditionals: Option<crate::durable::TransferWriteConditionals>,
    ) -> Result<ObjectManifest, ObjectFormatError> {
        let job = self
            .enqueue_stream(bucket, key, content_type, body, conditionals)
            .await?;
        self.wait_transfer(&job.id).await
    }

    pub async fn read_bytes(
        &self,
        bucket: &str,
        key: &str,
        range: Range<u64>,
    ) -> Result<Vec<u8>, ObjectFormatError> {
        let mut output = Vec::new();
        self.read_range_to_writer(bucket, key, range, &mut output)
            .await?;
        Ok(output)
    }

    pub async fn read_range_to_writer<W: Write>(
        &self,
        bucket: &str,
        key: &str,
        range: Range<u64>,
        writer: &mut W,
    ) -> Result<(), ObjectFormatError> {
        let manifest = self
            .metadata
            .get_active_manifest(bucket, key)?
            .ok_or_else(|| {
                ObjectFormatError::InvalidRead(format!("object not found: {bucket}/{key}"))
            })?;
        if manifest.commit_state != CommitState::Committed {
            return Err(ObjectFormatError::InvalidRead(format!(
                "object is not committed: {}/{}",
                bucket, key
            )));
        }
        let _pin = self.pin_object(manifest.object_id);
        let plan = Self::plan_read(&manifest, range.clone())?;
        for span in plan.chunks {
            let chunk = manifest.chunks.get(span.order as usize).ok_or(
                ObjectFormatError::MissingChunk {
                    object_id: manifest.object_id,
                    order: span.order,
                },
            )?;
            let message_id = i32::try_from(chunk.telegram_message_id).map_err(|_| {
                ObjectFormatError::InvalidRead(format!(
                    "telegram message id out of range for chunk {}",
                    chunk.order
                ))
            })?;
            let ciphertext = self.download_message_bytes(message_id).await?;
            let plaintext = if manifest.encryption.enabled {
                self.decrypt_chunk(manifest.object_id, chunk.order, &ciphertext)?
            } else {
                ciphertext
            };
            let actual_checksum = sha256_hex(&plaintext);
            if actual_checksum != chunk.checksum {
                return Err(ObjectFormatError::ChecksumMismatch {
                    scope: format!("chunk {}", chunk.order),
                    expected: chunk.checksum.clone(),
                    actual: actual_checksum,
                });
            }
            let start = span.offset_within_chunk as usize;
            let end = (span.offset_within_chunk + span.length) as usize;
            writer.write_all(&plaintext[start..end])?;
        }
        Ok(())
    }

    pub async fn reconcile(&self) -> Result<ReconciliationReport, ObjectFormatError> {
        let mut repaired_objects = 0_u64;
        let mut quarantined_objects = 0_u64;
        let mut orphaned_chunks = 0_u64;

        let manifests = self.metadata.list_manifests()?;
        let journal_entries = self.metadata.list_journal_entries()?;
        let journal_by_operation: HashMap<Uuid, JournalEntry> = journal_entries
            .into_iter()
            .map(|entry| (entry.operation_id, entry))
            .collect();
        let manifest_by_object: HashMap<Uuid, ObjectManifest> = manifests
            .iter()
            .cloned()
            .map(|manifest| (manifest.object_id, manifest))
            .collect();
        for manifest in &manifests {
            if self
                .metadata
                .transfer(&manifest.object_id.to_string())?
                .is_some()
            {
                continue;
            }
            match manifest.commit_state {
                CommitState::Staging => {
                    if let Some(entry) = journal_by_operation.values().find(|entry| {
                        entry.object_id == manifest.object_id && entry.state == "staging"
                    }) && self.staged_object_ready(entry.operation_id, manifest)?
                    {
                        let staged = StagedObject {
                            operation_id: entry.operation_id,
                            object_id: manifest.object_id,
                            manifest: manifest.clone(),
                            chunk_plan: self.plan_upload(manifest.content_length)?,
                        };
                        self.commit_staged_object(&staged).await?;
                        repaired_objects += 1;
                        continue;
                    }
                    self.metadata
                        .update_manifest_state(manifest.object_id, CommitState::RecoveryRequired)?;
                    quarantined_objects += self.quarantine_staging_manifest(manifest)?;
                }
                CommitState::Committed => {
                    if !self.committed_object_ready(manifest).await? {
                        self.metadata.update_manifest_state(
                            manifest.object_id,
                            CommitState::RecoveryRequired,
                        )?;
                        repaired_objects += self.try_repair_committed_manifest(manifest).await?;
                    }
                }
                CommitState::RecoveryRequired => {
                    if self.committed_object_ready(manifest).await? {
                        self.metadata
                            .update_manifest_state(manifest.object_id, CommitState::Committed)?;
                        repaired_objects += 1;
                    }
                }
                CommitState::Orphaned => {
                    quarantined_objects += 1;
                }
                CommitState::Tombstoned => {}
            }
        }

        for entry in fs::read_dir(self.data_dir.join(STAGING_ROOT))? {
            let entry = entry?;
            if !entry.file_type()?.is_dir() {
                continue;
            }
            let dir_name = entry.file_name().to_string_lossy().to_string();
            let operation_id = match Uuid::parse_str(&dir_name) {
                Ok(operation_id) => operation_id,
                Err(_)
                    if dir_name.starts_with("upload-")
                        && self
                            .metadata
                            .transfer(dir_name.trim_start_matches("upload-"))?
                            .is_some() =>
                {
                    continue;
                }
                Err(_) => {
                    self.quarantine_path(&entry.path())?;
                    quarantined_objects += 1;
                    continue;
                }
            };
            if journal_by_operation.contains_key(&operation_id) {
                continue;
            }
            self.quarantine_path(&entry.path())?;
            quarantined_objects += 1;
        }

        for entry in fs::read_dir(self.data_dir.join(MANIFEST_ROOT))? {
            let entry = entry?;
            if !entry.file_type()?.is_file() {
                continue;
            }
            let file_name = entry.file_name().to_string_lossy().to_string();
            let stem = file_name.trim_end_matches(".json");
            let object_id = match Uuid::parse_str(stem) {
                Ok(object_id) => object_id,
                Err(_) => {
                    self.quarantine_path(&entry.path())?;
                    quarantined_objects += 1;
                    continue;
                }
            };
            if !manifest_by_object.contains_key(&object_id) {
                self.quarantine_path(&entry.path())?;
                quarantined_objects += 1;
            }
        }

        for entry in fs::read_dir(self.data_dir.join(CHUNK_ROOT))? {
            let entry = entry?;
            if !entry.file_type()?.is_dir() {
                continue;
            }
            let dir_name = entry.file_name().to_string_lossy().to_string();
            let object_id = match Uuid::parse_str(&dir_name) {
                Ok(object_id) => object_id,
                Err(_) => {
                    self.quarantine_path(&entry.path())?;
                    orphaned_chunks += 1;
                    continue;
                }
            };
            if !manifest_by_object.contains_key(&object_id) {
                self.quarantine_path(&entry.path())?;
                orphaned_chunks += 1;
            }
        }

        let final_manifests = self.metadata.list_manifests()?;
        Ok(ReconciliationReport {
            staged_objects: final_manifests
                .iter()
                .filter(|manifest| manifest.commit_state == CommitState::Staging)
                .count() as u64,
            committed_objects: final_manifests
                .iter()
                .filter(|manifest| manifest.commit_state == CommitState::Committed)
                .count() as u64,
            recovery_required_objects: final_manifests
                .iter()
                .filter(|manifest| manifest.commit_state == CommitState::RecoveryRequired)
                .count() as u64,
            orphaned_chunks,
            repaired_objects,
            quarantined_objects,
        })
    }

    pub async fn garbage_collect(
        &self,
        dry_run: bool,
        retention: Duration,
    ) -> Result<GarbageCollectionReport, ObjectFormatError> {
        let cutoff = OffsetDateTime::now_utc() - retention;
        let tombstoned_manifests = self.metadata.list_tombstoned_manifests()?;
        let mut eligible_objects = 0_u64;
        let mut manifests_removed = 0_u64;
        let mut chunk_directories_removed = 0_u64;
        let mut quarantine_entries_removed = 0_u64;
        let mut bytes_removed = 0_u64;

        for record in tombstoned_manifests {
            if record.tombstoned_at > cutoff {
                continue;
            }
            eligible_objects += 1;
            let object_id = record.manifest.object_id;
            let manifest_path = self.manifest_path(object_id);
            if manifest_path.exists() {
                bytes_removed = bytes_removed.saturating_add(fs::metadata(&manifest_path)?.len());
                manifests_removed += 1;
            }

            let chunk_dir = self.chunk_dir(object_id);
            if chunk_dir.exists() {
                bytes_removed = bytes_removed.saturating_add(self.directory_size(&chunk_dir)?);
                chunk_directories_removed += 1;
            }

            bytes_removed = bytes_removed.saturating_add(record.manifest.content_length);
            if !dry_run {
                self.metadata.make_cleanup_due(&object_id.to_string())?;
                loop {
                    if self
                        .metadata
                        .cleanup_complete_for_object(&object_id.to_string())?
                    {
                        break;
                    }
                    let Some(target) = self.metadata.claim_cleanup()? else {
                        break;
                    };
                    self.process_cleanup_target(&target).await?;
                }
            }
            quarantine_entries_removed += self.cleanup_quarantine_entries(object_id, dry_run)?;
        }

        Ok(GarbageCollectionReport {
            dry_run,
            eligible_objects,
            manifests_removed,
            chunk_directories_removed,
            quarantine_entries_removed,
            bytes_removed,
        })
    }

    pub fn plan_read(
        manifest: &ObjectManifest,
        range: Range<u64>,
    ) -> Result<ReadPlan, ObjectFormatError> {
        if range.start > range.end {
            return Err(ObjectFormatError::InvalidRead(
                "range start is greater than range end".to_string(),
            ));
        }
        if range.end > manifest.content_length {
            return Err(ObjectFormatError::InvalidRead(format!(
                "range end exceeds content length: {} > {}",
                range.end, manifest.content_length
            )));
        }

        let mut chunks = Vec::new();
        for chunk in &manifest.chunks {
            let chunk_start = chunk.offset;
            let chunk_end = chunk.offset.checked_add(chunk.size).ok_or_else(|| {
                ObjectFormatError::InvalidRead("chunk offset overflow".to_string())
            })?;
            if chunk_end <= range.start || chunk_start >= range.end {
                continue;
            }
            let overlap_start = range.start.max(chunk_start);
            let overlap_end = range.end.min(chunk_end);
            chunks.push(ReadSpan {
                order: chunk.order,
                offset_within_chunk: overlap_start - chunk_start,
                length: overlap_end - overlap_start,
                path: PathBuf::new(),
                checksum: chunk.checksum.clone(),
            });
        }

        Ok(ReadPlan {
            object_id: manifest.object_id,
            requested_range: range,
            chunks,
        })
    }

    /// Emit one decrypted + checksum-verified slice per [`ReadSpan`], bounded by
    /// the chunk size and never allocating a whole object in memory.
    ///
    /// This is the single shared streaming reader used by both the S3
    /// `get_object` path and the `/_admin` download endpoint so their byte
    /// output stays identical. Errors surface per-chunk as stream items so a
    /// failure mid-download aborts the stream instead of buffering.
    pub fn read_spans_to_stream(
        this: Arc<Self>,
        manifest: &ObjectManifest,
        spans: Vec<ReadSpan>,
    ) -> impl futures::Stream<Item = Result<Bytes, io::Error>> + Send + 'static + use<> {
        let pin = this.pin_object(manifest.object_id);
        futures::stream::unfold(
            (
                Arc::clone(&this),
                manifest.clone(),
                0usize,
                spans,
                false,
                pin,
            ),
            move |(object_format, manifest, index, spans, done, pin)| async move {
                if done {
                    return None;
                }
                let span = spans.get(index)?.clone();
                let chunk = match manifest.chunks.get(span.order as usize) {
                    Some(chunk) => chunk,
                    None => {
                        return Some((
                            Err(io::Error::new(
                                io::ErrorKind::NotFound,
                                format!("missing chunk {}", span.order),
                            )),
                            (object_format, manifest, index + 1, spans, true, pin),
                        ));
                    }
                };
                let message_id = match i32::try_from(chunk.telegram_message_id) {
                    Ok(message_id) => message_id,
                    Err(_) => {
                        return Some((
                            Err(io::Error::new(
                                io::ErrorKind::InvalidData,
                                format!(
                                    "telegram message id out of range for chunk {}",
                                    span.order
                                ),
                            )),
                            (object_format, manifest, index + 1, spans, true, pin),
                        ));
                    }
                };
                let ciphertext = match object_format.download_message_bytes(message_id).await {
                    Ok(value) => value,
                    Err(error) => {
                        return Some((
                            Err(io::Error::other(error.to_string())),
                            (object_format, manifest, index + 1, spans, true, pin),
                        ));
                    }
                };
                let plaintext = if manifest.encryption.enabled {
                    match object_format.decrypt_chunk(manifest.object_id, span.order, &ciphertext) {
                        Ok(value) => value,
                        Err(error) => {
                            return Some((
                                Err(io::Error::new(
                                    io::ErrorKind::InvalidData,
                                    error.to_string(),
                                )),
                                (object_format, manifest, index + 1, spans, true, pin),
                            ));
                        }
                    }
                } else {
                    ciphertext
                };
                if sha256_hex(&plaintext) != span.checksum {
                    return Some((
                        Err(io::Error::new(
                            io::ErrorKind::InvalidData,
                            format!(
                                "checksum mismatch for chunk {}: expected {}, got {}",
                                span.order,
                                span.checksum,
                                sha256_hex(&plaintext)
                            ),
                        )),
                        (object_format, manifest, index + 1, spans, true, pin),
                    ));
                }
                let start = span.offset_within_chunk as usize;
                let end = start + span.length as usize;
                if plaintext.len() < end {
                    return Some((
                        Err(io::Error::new(
                            io::ErrorKind::UnexpectedEof,
                            format!(
                                "chunk {} shorter than planned span (len {}, need {}-{})",
                                span.order,
                                plaintext.len(),
                                start,
                                end
                            ),
                        )),
                        (object_format, manifest, index + 1, spans, true, pin),
                    ));
                }
                Some((
                    Ok(Bytes::copy_from_slice(&plaintext[start..end])),
                    (object_format, manifest, index + 1, spans, false, pin),
                ))
            },
        )
    }

    fn pin_object(&self, object_id: Uuid) -> ReadPinGuard {
        if let Ok(mut pins) = self.read_pins.lock() {
            *pins.entry(object_id).or_default() += 1;
        }
        ReadPinGuard {
            object_id,
            pins: Arc::clone(&self.read_pins),
        }
    }

    pub(super) fn is_read_pinned(&self, object_id: Uuid) -> bool {
        self.read_pins
            .lock()
            .ok()
            .and_then(|pins| pins.get(&object_id).copied())
            .unwrap_or(0)
            > 0
    }

    pub fn plan_chunks(
        content_length: u64,
        chunk_size: u64,
    ) -> Result<ChunkPlan, ObjectFormatError> {
        if chunk_size == 0 {
            return Err(ObjectFormatError::InvalidPlan(
                "chunk size must be non-zero".to_string(),
            ));
        }
        let mut chunks = Vec::new();
        let mut offset = 0_u64;
        let mut order = 0_u32;
        while offset < content_length {
            let remaining = content_length - offset;
            let size = remaining.min(chunk_size);
            chunks.push(PlannedChunk {
                order,
                offset,
                size,
            });
            offset = offset
                .checked_add(size)
                .ok_or_else(|| ObjectFormatError::InvalidPlan("offset overflow".to_string()))?;
            order = order.checked_add(1).ok_or_else(|| {
                ObjectFormatError::InvalidPlan("chunk order overflow".to_string())
            })?;
        }
        Ok(ChunkPlan {
            chunk_size,
            content_length,
            chunks,
        })
    }

    async fn committed_object_ready(
        &self,
        manifest: &ObjectManifest,
    ) -> Result<bool, ObjectFormatError> {
        for chunk in &manifest.chunks {
            let message_id = i32::try_from(chunk.telegram_message_id).map_err(|_| {
                ObjectFormatError::InvalidRead(format!(
                    "telegram message id out of range for chunk {}",
                    chunk.order
                ))
            })?;
            let bytes = self.download_message_bytes(message_id).await?;
            let plaintext = if manifest.encryption.enabled {
                self.decrypt_chunk(manifest.object_id, chunk.order, &bytes)?
            } else {
                bytes
            };
            if sha256_hex(&plaintext) != chunk.checksum {
                return Ok(false);
            }
        }
        Ok(true)
    }

    fn staged_object_ready(
        &self,
        operation_id: Uuid,
        manifest: &ObjectManifest,
    ) -> Result<bool, ObjectFormatError> {
        let staging_dir = self.staging_dir(operation_id);
        if !staging_dir.exists() {
            return Ok(false);
        }
        let stage_manifest = staging_dir.join(MANIFEST_FILE_NAME);
        if !stage_manifest.exists() {
            return Ok(false);
        }
        for chunk in &manifest.chunks {
            let path = staging_dir.join(chunk_file_name(chunk.order));
            if !path.exists() {
                return Ok(false);
            }
            let mut file = File::open(&path)?;
            let mut bytes = Vec::new();
            file.read_to_end(&mut bytes)?;
            let plaintext = if manifest.encryption.enabled {
                self.decrypt_chunk(manifest.object_id, chunk.order, &bytes)?
            } else {
                bytes
            };
            if sha256_hex(&plaintext) != chunk.checksum {
                return Ok(false);
            }
        }
        Ok(true)
    }

    async fn try_repair_committed_manifest(
        &self,
        manifest: &ObjectManifest,
    ) -> Result<u64, ObjectFormatError> {
        if self.committed_object_ready(manifest).await? {
            self.metadata
                .update_manifest_state(manifest.object_id, CommitState::Committed)?;
            return Ok(1);
        }
        Ok(0)
    }

    fn inspect_staging_manifest(
        &self,
        manifest: &ObjectManifest,
        operation_id: Option<Uuid>,
    ) -> Result<Vec<RecoveryIssue>, ObjectFormatError> {
        let mut issues = Vec::new();
        let mut missing_details = Vec::new();
        let mut corrupted_details = Vec::new();
        let mut invalid_details = Vec::new();

        if let Err(error) = manifest.validate() {
            invalid_details.push(format!("manifest validation failed: {error}"));
        }

        let staging_dir = operation_id
            .map(|id| self.staging_dir(id))
            .unwrap_or_else(|| {
                self.data_dir
                    .join(STAGING_ROOT)
                    .join(manifest.object_id.to_string())
            });

        if !staging_dir.exists() {
            issues.push(self.build_recovery_issue(
                manifest,
                "missing_staging_dir",
                "staging directory missing".to_string(),
                vec![format!(
                    "expected staging directory: {}",
                    staging_dir.display()
                )],
            ));
            return Ok(issues);
        }

        let stage_manifest = staging_dir.join(MANIFEST_FILE_NAME);
        if !stage_manifest.exists() {
            invalid_details.push(format!(
                "staged manifest missing: {}",
                stage_manifest.display()
            ));
        } else {
            let bytes = fs::read(&stage_manifest)?;
            let actual_checksum = sha256_hex(&bytes);
            let expected_checksum = sha256_hex(&serde_json::to_vec_pretty(manifest)?);
            if actual_checksum != expected_checksum {
                corrupted_details.push(format!(
                    "staged manifest checksum mismatch at {}: expected {}, got {}",
                    stage_manifest.display(),
                    expected_checksum,
                    actual_checksum
                ));
            }
        }

        for chunk in &manifest.chunks {
            let path = staging_dir.join(chunk_file_name(chunk.order));
            if !path.exists() {
                missing_details.push(format!(
                    "chunk {} missing at {}",
                    chunk.order,
                    path.display()
                ));
                continue;
            }
            let bytes = fs::read(&path)?;
            let plaintext = if manifest.encryption.enabled {
                match self.decrypt_chunk(manifest.object_id, chunk.order, &bytes) {
                    Ok(value) => value,
                    Err(error) => {
                        corrupted_details.push(format!(
                            "chunk {} could not be decrypted at {}: {}",
                            chunk.order,
                            path.display(),
                            error
                        ));
                        continue;
                    }
                }
            } else {
                bytes
            };
            let actual_checksum = sha256_hex(&plaintext);
            if actual_checksum != chunk.checksum {
                corrupted_details.push(format!(
                    "chunk {} checksum mismatch at {}: expected {}, got {}",
                    chunk.order,
                    path.display(),
                    chunk.checksum,
                    actual_checksum
                ));
            }
        }

        if !invalid_details.is_empty() {
            issues.push(self.build_recovery_issue(
                manifest,
                "invalid_staging_manifest",
                "staged metadata is invalid".to_string(),
                invalid_details,
            ));
        }
        if !missing_details.is_empty() {
            issues.push(self.build_recovery_issue(
                manifest,
                "missing_staged_chunk",
                format!("{} staged chunk(s) missing", missing_details.len()),
                missing_details,
            ));
        }
        if !corrupted_details.is_empty() {
            issues.push(self.build_recovery_issue(
                manifest,
                "corrupted_staged_chunk",
                format!("{} staged chunk(s) corrupted", corrupted_details.len()),
                corrupted_details,
            ));
        }

        Ok(issues)
    }

    async fn inspect_committed_manifest(
        &self,
        manifest: &ObjectManifest,
    ) -> Result<Vec<RecoveryIssue>, ObjectFormatError> {
        let mut issues = Vec::new();
        let mut missing_details = Vec::new();
        let mut corrupted_details = Vec::new();
        let mut invalid_details = Vec::new();

        if let Err(error) = manifest.validate() {
            invalid_details.push(format!("manifest validation failed: {error}"));
        }

        if manifest.chunks.is_empty() && manifest.content_length > 0 {
            invalid_details.push(format!(
                "manifest declares {} bytes but has no chunk references",
                manifest.content_length
            ));
        }

        for chunk in &manifest.chunks {
            let message_id = match i32::try_from(chunk.telegram_message_id) {
                Ok(message_id) => message_id,
                Err(_) => {
                    invalid_details.push(format!(
                        "chunk {} has telegram message id out of range: {}",
                        chunk.order, chunk.telegram_message_id
                    ));
                    continue;
                }
            };

            let ciphertext = match self.download_message_bytes(message_id).await {
                Ok(bytes) => bytes,
                Err(error) => {
                    let message = error.to_string();
                    if message.contains("not found") {
                        missing_details.push(format!(
                            "chunk {} missing from Telegram message {}",
                            chunk.order, message_id
                        ));
                    } else {
                        corrupted_details.push(format!(
                            "chunk {} could not be read from Telegram message {}: {}",
                            chunk.order, message_id, message
                        ));
                    }
                    continue;
                }
            };

            let plaintext = if manifest.encryption.enabled {
                match self.decrypt_chunk(manifest.object_id, chunk.order, &ciphertext) {
                    Ok(value) => value,
                    Err(error) => {
                        corrupted_details.push(format!(
                            "chunk {} could not be decrypted from Telegram message {}: {}",
                            chunk.order, message_id, error
                        ));
                        continue;
                    }
                }
            } else {
                ciphertext
            };

            let actual_checksum = sha256_hex(&plaintext);
            if actual_checksum != chunk.checksum {
                corrupted_details.push(format!(
                    "chunk {} checksum mismatch from Telegram message {}: expected {}, got {}",
                    chunk.order, message_id, chunk.checksum, actual_checksum
                ));
            }
        }

        if !invalid_details.is_empty() {
            issues.push(self.build_recovery_issue(
                manifest,
                "invalid_manifest",
                "manifest metadata is invalid".to_string(),
                invalid_details,
            ));
        }
        if !missing_details.is_empty() {
            issues.push(self.build_recovery_issue(
                manifest,
                "missing_chunk",
                format!("{} chunk(s) missing", missing_details.len()),
                missing_details,
            ));
        }
        if !corrupted_details.is_empty() {
            issues.push(self.build_recovery_issue(
                manifest,
                "corrupted_chunk",
                format!("{} chunk(s) corrupted", corrupted_details.len()),
                corrupted_details,
            ));
        }

        Ok(issues)
    }

    fn build_recovery_issue(
        &self,
        manifest: &ObjectManifest,
        kind: &str,
        summary: String,
        details: Vec<String>,
    ) -> RecoveryIssue {
        RecoveryIssue {
            object_id: Some(manifest.object_id),
            bucket: Some(manifest.bucket.clone()),
            key: Some(manifest.key.clone()),
            path: Some(format!("{}/{}", manifest.bucket, manifest.key)),
            commit_state: Some(manifest.commit_state),
            kind: kind.to_string(),
            summary,
            details,
        }
    }

    fn quarantine_staging_manifest(
        &self,
        manifest: &ObjectManifest,
    ) -> Result<u64, ObjectFormatError> {
        let mut quarantined = 0_u64;
        let staging_dir = self
            .metadata
            .list_journal_entries()?
            .into_iter()
            .find(|entry| entry.object_id == manifest.object_id)
            .map(|entry| self.staging_dir(entry.operation_id));
        if let Some(staging_dir) = staging_dir
            && staging_dir.exists()
        {
            self.quarantine_path(&staging_dir)?;
            quarantined += 1;
        }
        Ok(quarantined)
    }

    fn quarantine_path(&self, path: &Path) -> Result<(), ObjectFormatError> {
        let quarantine_path = self.quarantine_dir().join(
            path.file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .to_string(),
        );
        if quarantine_path.exists() {
            fs::remove_dir_all(&quarantine_path).ok();
            fs::remove_file(&quarantine_path).ok();
        }
        fs::rename(path, quarantine_path)?;
        Ok(())
    }

    fn cleanup_quarantine_entries(
        &self,
        object_id: Uuid,
        dry_run: bool,
    ) -> Result<u64, ObjectFormatError> {
        let quarantine_dir = self.quarantine_dir();
        if !quarantine_dir.exists() {
            return Ok(0);
        }
        let mut removed = 0_u64;
        let object_id = object_id.to_string();
        for entry in fs::read_dir(&quarantine_dir)? {
            let entry = entry?;
            let name = entry.file_name().to_string_lossy().to_string();
            if !name.contains(&object_id) {
                continue;
            }
            removed += 1;
            if dry_run {
                continue;
            }
            let file_type = entry.file_type()?;
            if file_type.is_dir() {
                fs::remove_dir_all(entry.path())?;
            } else {
                fs::remove_file(entry.path())?;
            }
        }
        Ok(removed)
    }

    fn directory_size(&self, path: &Path) -> Result<u64, ObjectFormatError> {
        let mut total = 0_u64;
        for entry in fs::read_dir(path)? {
            let entry = entry?;
            let metadata = entry.metadata()?;
            if metadata.is_file() {
                total = total.saturating_add(metadata.len());
            } else if metadata.is_dir() {
                total = total.saturating_add(self.directory_size(&entry.path())?);
            }
        }
        Ok(total)
    }

    fn encrypt_chunk(
        &self,
        object_id: Uuid,
        order: u32,
        plaintext: &[u8],
    ) -> Result<Vec<u8>, ObjectFormatError> {
        self.encryption.encrypt_chunk(object_id, order, plaintext)
    }

    pub(crate) fn decrypt_chunk(
        &self,
        object_id: Uuid,
        order: u32,
        ciphertext: &[u8],
    ) -> Result<Vec<u8>, ObjectFormatError> {
        self.encryption.decrypt_chunk(object_id, order, ciphertext)
    }

    fn staging_dir(&self, operation_id: Uuid) -> PathBuf {
        self.data_dir
            .join(STAGING_ROOT)
            .join(operation_id.to_string())
    }

    fn manifest_path(&self, object_id: Uuid) -> PathBuf {
        self.data_dir
            .join(MANIFEST_ROOT)
            .join(format!("{object_id}.json"))
    }

    fn chunk_dir(&self, object_id: Uuid) -> PathBuf {
        self.data_dir.join(CHUNK_ROOT).join(object_id.to_string())
    }

    fn ensure_connection_not_removing(&self) -> Result<(), ObjectFormatError> {
        if self.metadata.connection_removal_in_progress()? {
            return Err(ObjectFormatError::InvalidPlan(
                "Telegram connection removal is in progress".to_string(),
            ));
        }
        Ok(())
    }

    fn multipart_dir(&self, upload_id: Uuid) -> PathBuf {
        self.data_dir
            .join(MULTIPART_ROOT)
            .join(upload_id.to_string())
    }

    fn quarantine_dir(&self) -> PathBuf {
        self.data_dir.join(QUARANTINE_ROOT)
    }

    fn new_manifest(&self, args: ManifestBuildArgs) -> ObjectManifest {
        ObjectManifest {
            schema_version: 1,
            commit_state: args.commit_state,
            object_id: args.object_id,
            bucket: args.bucket,
            key: args.key,
            version_id: Some(args.object_id.to_string()),
            content_length: args.chunks.iter().map(|chunk| chunk.size).sum(),
            content_type: args.content_type,
            user_metadata: BTreeMap::new(),
            tags: BTreeMap::new(),
            created_at: OffsetDateTime::now_utc(),
            checksum: ObjectChecksum {
                algorithm: CHECKSUM_ALGORITHM.to_string(),
                whole_object: args.whole_checksum,
            },
            encryption: crate::manifest::EncryptionInfo {
                enabled: true,
                format: ENCRYPTION_FORMAT.to_string(),
                key_id: Some(self.encryption.key_id.clone()),
            },
            telegram: crate::manifest::TelegramLocation {
                peer_id: self.storage_chat_id().unwrap_or_default(),
                message_id: 0,
                document_id: Some(format!("local:{}:manifest", args.object_id)),
            },
            chunks: args.chunks,
        }
    }

    async fn upload_local_file_to_telegram(
        &self,
        path: &Path,
        file_name: &str,
    ) -> Result<TelegramLocation, ObjectFormatError> {
        let transport = self.transport_manager.current().await?;
        if transport.is_mock() {
            let message_id = self.next_mock_message_id()?;
            let mock_dir = self.mock_telegram_dir();
            fs::create_dir_all(&mock_dir)?;
            let destination = mock_dir.join(format!("{message_id}.bin"));
            fs::copy(path, &destination)?;
            fs::write(
                mock_dir.join(format!("{message_id}.json")),
                serde_json::to_vec(&serde_json::json!({ "file_name": file_name }))?,
            )?;
            return Ok(TelegramLocation {
                peer_id: self.storage_chat_id()?,
                message_id: i64::from(message_id),
                document_id: Some(format!("mock:{message_id}:{file_name}")),
            });
        }
        let client = transport.client()?;
        let storage_peer = transport.storage_peer().await?;
        let mut file = async_fs::File::open(path).await?;
        let size = file.metadata().await?.len();
        file.seek(std::io::SeekFrom::Start(0)).await?;
        let uploaded = client
            .upload_stream(&mut file, size as usize, file_name.to_string())
            .await
            .map_err(|error| {
                ObjectFormatError::Telegram(crate::telegram::TelegramTransportError::Rpc(format!(
                    "upload_file({file_name}) failed: {error}"
                )))
            })?;
        let message = client
            .send_message(
                storage_peer,
                InputMessage::new().text("").document(uploaded),
            )
            .await
            .map_err(|error| {
                ObjectFormatError::Telegram(crate::telegram::TelegramTransportError::Rpc(format!(
                    "send_message({file_name}) failed: {error}"
                )))
            })?;
        let media = message.media().ok_or_else(|| {
            ObjectFormatError::InvalidPlan(format!(
                "telegram upload did not return media for {file_name}"
            ))
        })?;
        let document = match media {
            Media::Document(document) => document,
            _ => {
                return Err(ObjectFormatError::InvalidPlan(format!(
                    "telegram upload did not store {file_name} as a document"
                )));
            }
        };
        Ok(TelegramLocation {
            peer_id: self.storage_chat_id()?,
            message_id: i64::from(message.id()),
            document_id: Some(document.id().to_string()),
        })
    }

    async fn download_message_bytes(&self, message_id: i32) -> Result<Vec<u8>, ObjectFormatError> {
        let transport = self.transport_manager.current().await?;
        if transport.is_mock() {
            let path = self.mock_telegram_dir().join(format!("{message_id}.bin"));
            if !path.exists() {
                return Err(ObjectFormatError::InvalidRead(format!(
                    "telegram message not found: {message_id}"
                )));
            }
            return Ok(fs::read(path)?);
        }
        let client = transport.client()?;
        let storage_peer = transport.storage_peer().await?;
        let messages = client
            .get_messages_by_id(storage_peer, &[message_id])
            .await
            .map_err(|error| {
                ObjectFormatError::Telegram(crate::telegram::TelegramTransportError::Rpc(
                    error.to_string(),
                ))
            })?;
        let message = messages.into_iter().flatten().next().ok_or_else(|| {
            ObjectFormatError::InvalidRead(format!("telegram message not found: {message_id}"))
        })?;
        let media = message.media().ok_or_else(|| {
            ObjectFormatError::InvalidRead(format!("telegram message has no media: {message_id}"))
        })?;
        let mut download = client.iter_download(&media);
        let mut bytes = Vec::new();
        while let Some(chunk) = download.next().await.map_err(|error| {
            ObjectFormatError::Telegram(crate::telegram::TelegramTransportError::Rpc(
                error.to_string(),
            ))
        })? {
            bytes.extend_from_slice(&chunk);
        }
        Ok(bytes)
    }

    async fn delete_telegram_messages(&self, message_ids: &[i32]) -> Result<(), ObjectFormatError> {
        if message_ids.is_empty() {
            return Ok(());
        }
        let transport = self.transport_manager.current().await?;
        if transport.is_mock() {
            let mock_dir = self.mock_telegram_dir();
            for message_id in message_ids {
                let path = mock_dir.join(format!("{message_id}.bin"));
                if path.exists() {
                    fs::remove_file(path)?;
                }
                let sidecar = mock_dir.join(format!("{message_id}.json"));
                if sidecar.exists() {
                    fs::remove_file(sidecar)?;
                }
            }
            return Ok(());
        }
        let client = transport.client()?;
        let storage_peer = transport.storage_peer().await?;
        client
            .delete_messages(storage_peer, message_ids)
            .await
            .map_err(|error| {
                ObjectFormatError::Telegram(crate::telegram::TelegramTransportError::Rpc(
                    error.to_string(),
                ))
            })?;
        Ok(())
    }

    fn mock_telegram_dir(&self) -> PathBuf {
        self.data_dir.join(MOCK_TELEGRAM_ROOT)
    }

    fn next_mock_message_id(&self) -> Result<i32, ObjectFormatError> {
        let mock_dir = self.mock_telegram_dir();
        fs::create_dir_all(&mock_dir)?;
        let mut observed_max = 0_i32;
        for entry in fs::read_dir(&mock_dir)? {
            let entry = entry?;
            let stem = entry
                .path()
                .file_stem()
                .and_then(|value| value.to_str())
                .and_then(|value| value.parse::<i32>().ok());
            if let Some(message_id) = stem
                && message_id > observed_max
            {
                observed_max = message_id;
            }
        }
        Ok(self.metadata.allocate_mock_message_id(observed_max)?)
    }
}

fn verify_staged_chunks(
    staging_dir: &Path,
    manifest: &ObjectManifest,
    encryption: &ObjectEncryption,
) -> Result<(), ObjectFormatError> {
    for chunk in &manifest.chunks {
        let path = staging_dir.join(chunk_file_name(chunk.order));
        if !path.exists() {
            return Err(ObjectFormatError::MissingChunk {
                object_id: manifest.object_id,
                order: chunk.order,
            });
        }
        let mut file = File::open(&path)?;
        let mut bytes = Vec::new();
        file.read_to_end(&mut bytes)?;
        let plaintext = if manifest.encryption.enabled {
            encryption.decrypt_chunk(manifest.object_id, chunk.order, &bytes)?
        } else {
            bytes
        };
        let actual_checksum = sha256_hex(&plaintext);
        if actual_checksum != chunk.checksum {
            return Err(ObjectFormatError::ChecksumMismatch {
                scope: format!("chunk {}", chunk.order),
                expected: chunk.checksum.clone(),
                actual: actual_checksum,
            });
        }
    }

    let committed_manifest_path = staging_dir.join(MANIFEST_FILE_NAME);
    if committed_manifest_path.exists() {
        let mut file = File::open(&committed_manifest_path)?;
        let mut bytes = Vec::new();
        file.read_to_end(&mut bytes)?;
        let actual_checksum = sha256_hex(&bytes);
        let expected = sha256_hex(&serde_json::to_vec_pretty(manifest)?);
        if actual_checksum != expected {
            return Err(ObjectFormatError::ChecksumMismatch {
                scope: "staged manifest".to_string(),
                expected,
                actual: actual_checksum,
            });
        }
    }

    Ok(())
}

fn write_json_file(path: &Path, value: &ObjectManifest) -> Result<(), ObjectFormatError> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let temporary = path.with_extension("json.tmp");
    let mut file = File::create(&temporary)?;
    let bytes = serde_json::to_vec_pretty(value)?;
    file.write_all(&bytes)?;
    file.sync_all()?;
    drop(file);
    fs::rename(&temporary, path)?;
    if let Some(parent) = path.parent() {
        workflow::sync_directory(parent)?;
    }
    Ok(())
}

pub fn sha256_hex(bytes: impl AsRef<[u8]>) -> String {
    let digest = Sha256::digest(bytes.as_ref());
    hex::encode(digest)
}

pub fn parse_checksum_hex(value: &str) -> Result<Vec<u8>, ObjectFormatError> {
    hex::decode(value).map_err(|error| ObjectFormatError::InvalidChecksum(error.to_string()))
}

fn chunk_file_name(order: u32) -> String {
    format!("chunk-{order:08}.bin")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::AppConfig;
    use crate::manifest::CommittedManifestArgs;
    use crate::metadata::TelegramBootstrapSettings;
    use crate::multipart::{MultipartCompletionPlan, MultipartPartPlan};
    use crate::telegram::{
        TelegramConnectionHealth, TelegramConnectionState, TelegramTransportManager,
    };
    use bytes::Bytes;
    use std::env;
    use std::sync::atomic::Ordering;
    use tempfile::TempDir;
    use time::Duration;

    fn test_config(tempdir: &TempDir) -> AppConfig {
        AppConfig {
            telegram_metadata_path: Some(
                tempdir.path().join("metadata.sqlite").display().to_string(),
            ),
            telegram_data_dir: Some(tempdir.path().join("data").display().to_string()),
            telegram_s3_master_key: Some("master-key".to_string()),
            rustfs_access_key: Some("access-key".to_string()),
            rustfs_secret_key: Some("secret-key".to_string()),
            telegram_admin_bootstrap_secret: Some("admin-secret".to_string()),
            telegram_admin_bind_addr: Some("127.0.0.1:9001".to_string()),
            telegram_s3_bind_addr: Some("127.0.0.1:9000".to_string()),
            ..AppConfig::default()
        }
    }

    fn seed_telegram_settings(tempdir: &TempDir) {
        let store = MetadataStore::open(tempdir.path().join("metadata.sqlite")).expect("metadata");
        store
            .set_telegram_bootstrap_settings(&TelegramBootstrapSettings {
                telegram_api_id: Some("123456".to_string()),
                telegram_api_hash: Some("test-api-hash".to_string()),
                telegram_storage_chat_id: Some("-1001234567890".to_string()),
                telegram_proxy_mode: Some("auto".to_string()),
                ..TelegramBootstrapSettings::default()
            })
            .expect("telegram settings");
    }

    async fn sample_service(tempdir: &TempDir) -> ObjectFormatService {
        unsafe {
            env::set_var("TELEGRAM_TRANSPORT_RUNTIME", "mock");
        }
        seed_telegram_settings(tempdir);
        let service = ObjectFormatService::open(&test_config(tempdir))
            .await
            .expect("service");
        if !service
            .bucket_exists("bucket")
            .expect("inspect test bucket")
        {
            service.create_bucket("bucket").expect("create test bucket");
        }
        service
    }

    #[test]
    fn chunk_plans_split_content_into_contiguous_ranges() {
        let plan = ObjectFormatService::plan_chunks(4097, 1024).expect("plan");
        assert_eq!(plan.chunks.len(), 5);
        assert_eq!(plan.chunks[0].offset, 0);
        assert_eq!(plan.chunks[4].size, 1);
    }

    #[test]
    fn checksum_helpers_round_trip_hex() {
        let checksum = sha256_hex(b"hello world");
        let decoded = parse_checksum_hex(&checksum).expect("checksum");
        assert_eq!(decoded.len(), 32);
    }

    #[tokio::test]
    async fn put_and_read_range_are_chunk_aware() {
        let tempdir = TempDir::new().expect("tempdir");
        let service = sample_service(&tempdir).await;
        let payload = b"abcdefghijklmnopqrstuvwxyz";
        let manifest = service
            .put_bytes("bucket", "key.txt", "text/plain", payload)
            .await
            .expect("put");
        assert_eq!(manifest.commit_state, CommitState::Committed);
        let bytes = service
            .read_bytes("bucket", "key.txt", 3..19)
            .await
            .expect("read");
        assert_eq!(&bytes, b"defghijklmnopqrs");
        assert!(!service.chunk_path(manifest.object_id, 0).exists());
        assert!(!service.manifest_file_path(manifest.object_id).exists());
        assert!(tempdir.path().join("data/mock-telegram/1.bin").exists());
    }

    #[tokio::test]
    async fn staged_upload_can_be_reconciled_after_restart() {
        let tempdir = TempDir::new().expect("tempdir");
        let service = sample_service(&tempdir).await;

        let staged = service
            .stage_bytes("bucket", "key.txt", "text/plain", b"hello world")
            .expect("stage");
        assert_eq!(staged.manifest.commit_state, CommitState::Staging);
        drop(service);
        let service = sample_service(&tempdir).await;
        service
            .wait_transfer(&staged.object_id.to_string())
            .await
            .expect("resume queued upload");
        let report = service.reconcile().await.expect("reconcile");
        assert_eq!(report.committed_objects, 1);
        assert_eq!(report.staged_objects, 0);
    }

    #[tokio::test]
    async fn multipart_completion_is_idempotent_and_abort_closes_upload() {
        let tempdir = TempDir::new().expect("tempdir");
        let service = sample_service(&tempdir).await;

        let upload = service
            .initiate_multipart_upload("bucket", "multipart.txt", "text/plain", Some("sha256"))
            .expect("initiate multipart");
        let part = service
            .upload_multipart_part(
                upload.upload_id,
                1,
                Some(s3s::dto::StreamingBlob::from_bytes(Bytes::from_static(
                    b"hello multipart",
                ))),
                None,
            )
            .await
            .expect("upload part");
        let plan = MultipartCompletionPlan {
            upload_id: upload.upload_id,
            object_id: upload.upload_id,
            bucket: upload.bucket.clone(),
            key: upload.key.clone(),
            content_type: upload.content_type.clone(),
            checksum_algorithm: upload.checksum_algorithm.clone(),
            content_length: part.size,
            parts: vec![MultipartPartPlan {
                part_number: 1,
                offset: 0,
                size: part.size,
                checksum: part.e_tag.clone(),
                e_tag: part.e_tag.clone(),
            }],
        };

        let completed = service
            .complete_multipart_upload(plan.clone())
            .await
            .expect("complete multipart");
        let repeat = service
            .complete_multipart_upload(plan)
            .await
            .expect("repeat complete");
        assert_eq!(completed.object_id, repeat.object_id);
        assert_eq!(
            completed.checksum.whole_object,
            repeat.checksum.whole_object
        );

        let aborted = service
            .initiate_multipart_upload("bucket", "aborted.txt", "text/plain", Some("sha256"))
            .expect("initiate aborted multipart");
        let _ = service
            .upload_multipart_part(
                aborted.upload_id,
                1,
                Some(s3s::dto::StreamingBlob::from_bytes(Bytes::from_static(
                    b"aborted part",
                ))),
                None,
            )
            .await
            .expect("upload aborted part");
        service
            .abort_multipart_upload(aborted.upload_id)
            .await
            .expect("abort multipart");

        assert!(
            service
                .upload_multipart_part(
                    aborted.upload_id,
                    2,
                    Some(s3s::dto::StreamingBlob::from_bytes(Bytes::from_static(
                        b"late part",
                    ))),
                    None,
                )
                .await
                .is_err(),
            "aborted multipart upload should reject additional parts"
        );
        assert!(
            service
                .complete_multipart_upload(MultipartCompletionPlan {
                    upload_id: aborted.upload_id,
                    object_id: aborted.upload_id,
                    bucket: aborted.bucket.clone(),
                    key: aborted.key.clone(),
                    content_type: aborted.content_type.clone(),
                    checksum_algorithm: aborted.checksum_algorithm.clone(),
                    content_length: 0,
                    parts: vec![MultipartPartPlan {
                        part_number: 1,
                        offset: 0,
                        size: 0,
                        checksum: "ignored".to_string(),
                        e_tag: "ignored".to_string(),
                    }],
                })
                .await
                .is_err(),
            "aborted multipart upload should not complete"
        );
    }

    #[tokio::test]
    async fn bootstrap_defers_remote_reconciliation_without_telegram_settings() {
        let tempdir = TempDir::new().expect("tempdir");
        unsafe {
            env::set_var("TELEGRAM_TRANSPORT_RUNTIME", "mock");
        }
        let service = ObjectFormatService::open(&test_config(&tempdir))
            .await
            .expect("service");
        let manifest = ObjectManifest::committed(CommittedManifestArgs {
            bucket: "bucket".to_string(),
            key: "old.txt".to_string(),
            content_length: 3,
            content_type: "text/plain".to_string(),
            checksum_algorithm: CHECKSUM_ALGORITHM.to_string(),
            whole_object: sha256_hex(b"old"),
            peer_id: "-1001234567890".to_string(),
            message_id: 1,
        });
        let operation_id = service
            .metadata
            .stage_manifest(OperationKind::Put, manifest)
            .expect("stage manifest");
        service
            .metadata
            .commit_manifest(operation_id)
            .expect("commit manifest");

        let status = service.bootstrap().await.expect("bootstrap status");
        assert_eq!(status.committed_objects, 1);
        assert_eq!(status.recovery_required_objects, 0);
    }

    #[tokio::test]
    async fn bootstrap_returns_status_when_telegram_is_disconnected() {
        let tempdir = TempDir::new().expect("tempdir");
        unsafe {
            env::set_var("TELEGRAM_TRANSPORT_RUNTIME", "mock");
        }
        seed_telegram_settings(&tempdir);
        let config = test_config(&tempdir);
        let transport = crate::telegram::TelegramTransport::open(config.clone())
            .await
            .expect("transport");
        let status = transport.status().await.expect("status");
        let health = TelegramConnectionHealth {
            status: status.clone(),
            state: TelegramConnectionState::Disconnected,
            detail: "storage peer lookup failed: not reachable".to_string(),
            checked_at: OffsetDateTime::now_utc().unix_timestamp(),
            last_success_at: None,
        };
        let transport_manager = TelegramTransportManager::from_parts(
            config.clone(),
            Some(std::sync::Arc::new(transport)),
            health,
        );
        let service = ObjectFormatService::open_with_transport_manager(&config, transport_manager)
            .await
            .expect("service");

        let manifest = ObjectManifest::committed(CommittedManifestArgs {
            bucket: "bucket".to_string(),
            key: "old.txt".to_string(),
            content_length: 3,
            content_type: "text/plain".to_string(),
            checksum_algorithm: CHECKSUM_ALGORITHM.to_string(),
            whole_object: sha256_hex(b"old"),
            peer_id: "-1001234567890".to_string(),
            message_id: 1,
        });
        let operation_id = service
            .metadata
            .stage_manifest(OperationKind::Put, manifest)
            .expect("stage manifest");
        service
            .metadata
            .commit_manifest(operation_id)
            .expect("commit manifest");

        let status = service.bootstrap().await.expect("bootstrap status");
        assert_eq!(status.committed_objects, 1);
        assert_eq!(status.recovery_required_objects, 0);
    }

    #[tokio::test]
    async fn recovery_snapshot_refreshes_cached_issue_list() {
        let tempdir = TempDir::new().expect("tempdir");
        let service = sample_service(&tempdir).await;
        let orphan_dir = tempdir
            .path()
            .join("data")
            .join(STAGING_ROOT)
            .join("manual-orphan");
        std::fs::create_dir_all(&orphan_dir).expect("orphan dir");

        service
            .refresh_recovery_snapshot()
            .await
            .expect("refresh snapshot");
        let snapshot = service.cached_recovery_snapshot().expect("snapshot");
        assert!(snapshot.0.is_some());
        assert!(
            snapshot
                .1
                .iter()
                .any(|issue| issue.kind == "orphaned_staging_dir")
        );
        assert!(snapshot.2.is_none());
    }

    #[tokio::test]
    async fn worker_runtime_can_restart_after_shutdown() {
        let tempdir = TempDir::new().expect("tempdir");
        let service = sample_service(&tempdir).await;

        service.ensure_workers();
        assert!(service.worker_runtime.started.load(Ordering::SeqCst));
        service.ensure_workers();

        service.shutdown_workers().await;
        assert!(!service.worker_runtime.started.load(Ordering::SeqCst));

        service.ensure_workers();
        assert!(service.worker_runtime.started.load(Ordering::SeqCst));

        service.shutdown_workers().await;
        assert!(!service.worker_runtime.started.load(Ordering::SeqCst));
    }

    #[tokio::test]
    async fn encrypted_writes_round_trip_and_change_at_rest_bytes() {
        let tempdir = TempDir::new().expect("tempdir");
        let service = sample_service(&tempdir).await;
        let payload = b"hello encrypted world";

        let manifest = service
            .put_bytes("bucket", "encrypted.txt", "text/plain", payload)
            .await
            .expect("put");

        assert!(manifest.encryption.enabled);
        assert_eq!(manifest.encryption.format, ENCRYPTION_FORMAT);
        assert!(manifest.encryption.key_id.is_some());
        assert_eq!(
            service
                .read_bytes("bucket", "encrypted.txt", 0..payload.len() as u64)
                .await
                .expect("read"),
            payload
        );

        let stored = fs::read(tempdir.path().join("data/mock-telegram/1.bin")).expect("mock blob");
        assert_ne!(stored, payload);
    }

    #[tokio::test]
    async fn garbage_collection_removes_aged_tombstoned_artifacts() {
        let tempdir = TempDir::new().expect("tempdir");
        let service = sample_service(&tempdir).await;
        let manifest = service
            .put_bytes("bucket", "gc.txt", "text/plain", b"gc payload")
            .await
            .expect("put");
        let tombstoned = service
            .tombstone_manifest(manifest.object_id, "cleanup test")
            .expect("tombstone");
        assert_eq!(tombstoned.commit_state, CommitState::Tombstoned);

        let preview = service
            .garbage_collect(true, Duration::seconds(0))
            .await
            .expect("dry-run gc");
        assert_eq!(preview.eligible_objects, 1);

        let report = service
            .garbage_collect(false, Duration::seconds(0))
            .await
            .expect("gc");
        assert_eq!(report.eligible_objects, 1);
        assert!(report.bytes_removed >= b"gc payload".len() as u64);
        assert!(!service.manifest_file_path(manifest.object_id).exists());
        assert!(!service.chunk_path(manifest.object_id, 0).exists());
        assert!(
            service
                .read_bytes("bucket", "gc.txt", 0..b"gc payload".len() as u64)
                .await
                .is_err()
        );
    }

    #[tokio::test]
    async fn connection_removal_keeps_owner_until_remote_cleanup_finishes() {
        let tempdir = TempDir::new().expect("tempdir");
        let service = sample_service(&tempdir).await;
        let manifest = service
            .put_bytes("bucket", "removed.txt", "text/plain", b"remove me")
            .await
            .expect("put");
        let job = service
            .metadata
            .begin_connection_removal(true)
            .expect("begin removal");

        service.ensure_workers();
        for _ in 0..100 {
            if service
                .metadata
                .connection_removal_job()
                .expect("job")
                .is_none()
            {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
        service.shutdown_workers().await;

        assert!(
            service
                .metadata
                .connection_removal_job()
                .expect("job")
                .is_none(),
            "removal job {} did not finish",
            job.id
        );
        assert!(
            service
                .metadata
                .active_connection_id()
                .expect("active connection")
                .is_none()
        );
        assert!(!service.manifest_file_path(manifest.object_id).exists());
        assert!(!service.chunk_path(manifest.object_id, 0).exists());
        assert!(!tempdir.path().join("data/mock-telegram/1.bin").exists());
        assert!(!tempdir.path().join("data/mock-telegram/2.bin").exists());
    }
}
