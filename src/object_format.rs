mod reception;
mod workflow;
use crate::config::AppConfig;
use crate::manifest::{
    ChunkRef, ChunkReplica, CommitState, MANIFEST_SCHEMA_VERSION, ObjectChecksum, ObjectManifest,
    ReplicaMode, TelegramLocation,
};
use crate::metadata::{
    BucketRecord, JournalEntry, MetadataError, MetadataStatus, MetadataStore, OperationKind,
    TrafficCounterKind, TrafficTotals,
};
use crate::multipart::{MultipartCompletionPlan, MultipartPart, MultipartSession, MultipartState};
use crate::telegram::{
    RetryDecision, RetryPolicy, TelegramConnectionState, TelegramTransportError,
    TelegramTransportManager, parse_flood_wait_seconds,
};
use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use bytes::Bytes;
use futures::StreamExt;
use grammers_client::media::Media;
use grammers_client::message::InputMessage;
use rand_core::{OsRng, RngCore};
use ring::aead::{Aad, CHACHA20_POLY1305, LessSafeKey, Nonce, UnboundKey};
use ring::rand::{SecureRandom, SystemRandom};
use s3s::dto::StreamingBlob;
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, HashMap, VecDeque};
use std::fs::{self, File};
use std::io::{self, Read, Write};
use std::ops::Range;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, RwLock};
use std::time::{Duration as StdDuration, Instant as StdInstant};
use thiserror::Error;
use time::{Duration, OffsetDateTime};
use tokio::fs as async_fs;
use tokio::io::{AsyncSeekExt, AsyncWriteExt};
use tracing::warn;
use uuid::Uuid;

pub(crate) type RecoverySnapshot = (Option<i64>, Vec<RecoveryIssue>, Option<String>);

const CHECKSUM_ALGORITHM: &str = "sha256";
const MULTIPART_CHECKSUM_ALGORITHM: &str = "sha256-parts-v1";
const ENCRYPTION_FORMAT: &str = "chacha20poly1305-v1";
/// Local tombstones and orphaned cleanup material are retained for one day
/// before irreversible garbage collection becomes eligible.
pub const GARBAGE_COLLECTION_RETENTION_SECONDS: i64 = 24 * 60 * 60;
const MANIFEST_FILE_NAME: &str = "manifest.json";
const STAGING_ROOT: &str = "staging";
const MANIFEST_ROOT: &str = "manifests";
const CHUNK_ROOT: &str = "chunks";
const QUARANTINE_ROOT: &str = "quarantine";
const MULTIPART_ROOT: &str = "multipart";
const MOCK_TELEGRAM_ROOT: &str = "mock-telegram";
const CLEANUP_EVIDENCE_ROOT: &str = "cleanup-evidence";
pub const TELEGRAM_STREAM_RECOVERY_WINDOW_SECS: u64 = 120;
const TELEGRAM_STREAM_RECOVERY_WINDOW: StdDuration =
    StdDuration::from_secs(TELEGRAM_STREAM_RECOVERY_WINDOW_SECS);
pub const RESERVED_BUCKET_NAMES: [&str; 2] = ["_public", "_admin"];

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
    /// Logical bytes represented by unique committed Telegram chunk files.
    /// Telegram protocol/envelope overhead is not part of the manifest data.
    pub telegram_files_bytes: u64,
}

/// Lightweight account health used by the admin overview. It intentionally
/// contains no credentials or session paths.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct TelegramAccountHealthSnapshot {
    pub id: String,
    pub label: String,
    pub state: String,
    pub detail: String,
    pub connected: bool,
    pub download_enabled: bool,
}

fn telegram_files_bytes(manifests: &[ObjectManifest]) -> u64 {
    let mut telegram_files = HashMap::new();
    for manifest in manifests
        .iter()
        .filter(|manifest| manifest.commit_state == CommitState::Committed)
    {
        for chunk in &manifest.chunks {
            let key = (
                chunk.telegram_peer_id.clone(),
                chunk.telegram_message_id,
                chunk.telegram_document_id.clone(),
            );
            telegram_files.entry(key).or_insert(chunk.size);
        }
    }
    telegram_files.values().copied().sum()
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
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

    fn share_key(&self) -> [u8; 32] {
        let mut hasher = Sha256::new();
        hasher.update(self.key);
        hasher.update(b"telegram-s3-share-token-v1");
        let digest = hasher.finalize();
        let mut key = [0_u8; 32];
        key.copy_from_slice(&digest);
        key
    }

    fn encrypt_share_token(&self, token: &str) -> Result<String, ObjectFormatError> {
        let unbound = UnboundKey::new(&CHACHA20_POLY1305, &self.share_key())
            .map_err(|_| ObjectFormatError::InvalidChecksum("invalid share key".to_string()))?;
        let cipher = LessSafeKey::new(unbound);
        let mut nonce_bytes = [0_u8; 12];
        SystemRandom::new()
            .fill(&mut nonce_bytes)
            .map_err(|_| ObjectFormatError::InvalidChecksum("random nonce failed".to_string()))?;
        let nonce = Nonce::try_assume_unique_for_key(&nonce_bytes)
            .map_err(|_| ObjectFormatError::InvalidChecksum("invalid share nonce".to_string()))?;
        let mut encrypted = token.as_bytes().to_vec();
        cipher
            .seal_in_place_append_tag(
                nonce,
                Aad::from(b"telegram-s3-share-token-v1"),
                &mut encrypted,
            )
            .map_err(|_| {
                ObjectFormatError::InvalidChecksum("share token encryption failed".to_string())
            })?;
        let mut payload = nonce_bytes.to_vec();
        payload.extend_from_slice(&encrypted);
        Ok(URL_SAFE_NO_PAD.encode(payload))
    }

    fn decrypt_share_token(&self, encoded: &str) -> Result<String, ObjectFormatError> {
        let payload = URL_SAFE_NO_PAD.decode(encoded).map_err(|_| {
            ObjectFormatError::InvalidChecksum("invalid share token encoding".to_string())
        })?;
        if payload.len() < 12 + CHACHA20_POLY1305.tag_len() {
            return Err(ObjectFormatError::InvalidChecksum(
                "invalid share token payload".to_string(),
            ));
        }
        let nonce =
            Nonce::try_assume_unique_for_key(payload[..12].try_into().map_err(|_| {
                ObjectFormatError::InvalidChecksum("invalid share nonce".to_string())
            })?)
            .map_err(|_| ObjectFormatError::InvalidChecksum("invalid share nonce".to_string()))?;
        let unbound = UnboundKey::new(&CHACHA20_POLY1305, &self.share_key())
            .map_err(|_| ObjectFormatError::InvalidChecksum("invalid share key".to_string()))?;
        let cipher = LessSafeKey::new(unbound);
        let mut encrypted = payload[12..].to_vec();
        let plaintext = cipher
            .open_in_place(
                nonce,
                Aad::from(b"telegram-s3-share-token-v1"),
                &mut encrypted,
            )
            .map_err(|_| {
                ObjectFormatError::InvalidChecksum("share token decryption failed".to_string())
            })?;
        String::from_utf8(plaintext.to_vec())
            .map_err(|_| ObjectFormatError::InvalidChecksum("share token is not utf-8".to_string()))
    }
}

struct ManifestBuildArgs {
    object_id: Uuid,
    bucket: String,
    key: String,
    content_type: String,
    expires_at: Option<OffsetDateTime>,
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
    #[error("invalid bucket name: {0}")]
    InvalidBucketName(String),
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
    config: Option<AppConfig>,
    transport_manager: std::sync::Arc<TelegramTransportManager>,
    data_dir: PathBuf,
    chunk_size: Arc<RwLock<u64>>,
    download_prefetch_chunks: Arc<RwLock<u64>>,
    recovery_verify_enabled: Arc<RwLock<bool>>,
    recovery_verify_interval_secs: Arc<RwLock<u64>>,
    recovery_verify_chunks: Arc<RwLock<u64>>,
    storage_chat_id: Arc<RwLock<String>>,
    worker_runtime: Arc<WorkerRuntime>,
    read_pins: Arc<Mutex<HashMap<Uuid, u64>>>,
    receptions: Arc<Mutex<HashMap<Uuid, Arc<tokio::sync::Mutex<reception::ReceptionState>>>>>,
    recovery_snapshot: Arc<RwLock<RecoverySnapshot>>,
    staging_budget: u64,
    encryption: ObjectEncryption,
    traffic: Arc<TrafficCounters>,
    download_stage_metrics: Arc<DownloadStageMetricsStore>,
    account_managers: Arc<Mutex<HashMap<String, Arc<TelegramTransportManager>>>>,
}

/// Payload counters displayed by the operator overview.
#[derive(Debug, Clone, Default, serde::Serialize)]
pub struct TrafficMetrics {
    pub session: TrafficSnapshot,
    pub total: TrafficSnapshot,
}

#[derive(Debug, Clone, Default, serde::Serialize)]
pub struct TrafficSnapshot {
    pub client_upload_bytes: u64,
    pub client_download_bytes: u64,
    pub telegram_upload_bytes: u64,
    pub telegram_download_bytes: u64,
}

/// Bounded read-stage diagnostics exposed to the authenticated Overview.
/// Samples are intentionally process-local; they are for live troubleshooting,
/// not durable accounting or recovery state.
#[derive(Debug, Clone, Default, serde::Serialize)]
pub struct DownloadStageMetrics {
    pub active_requests: u64,
    pub completed_requests: u64,
    pub failed_requests: u64,
    pub test_active_requests: u64,
    pub last_test: Option<DownloadStageSample>,
    pub recent: Vec<DownloadStageSample>,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct DownloadStageSample {
    pub request_id: u64,
    pub surface: String,
    pub started_at: String,
    pub status: String,
    pub chunks: u64,
    pub client_bytes: u64,
    pub telegram_bytes: u64,
    pub telegram_retries: u64,
    pub first_chunk_us: Option<u64>,
    pub telegram_us: u64,
    pub retry_wait_us: u64,
    pub decrypt_us: u64,
    pub verify_us: u64,
    pub total_us: u64,
    pub error: Option<String>,
}

#[derive(Default)]
struct DownloadStageMetricsStore {
    next_request_id: AtomicU64,
    active_requests: AtomicU64,
    completed_requests: AtomicU64,
    failed_requests: AtomicU64,
    test_active_requests: AtomicU64,
    last_test: Mutex<Option<DownloadStageSample>>,
    recent: Mutex<VecDeque<DownloadStageSample>>,
}

struct DownloadStageAccumulator {
    request_id: u64,
    surface: &'static str,
    started: StdInstant,
    started_at: String,
    chunks: u64,
    client_bytes: u64,
    telegram_bytes: u64,
    telegram_retries: u64,
    first_chunk_us: Option<u64>,
    telegram_us: u64,
    retry_wait_us: u64,
    decrypt_us: u64,
    verify_us: u64,
}

struct DownloadStageGuard {
    store: Arc<DownloadStageMetricsStore>,
    sample: Option<DownloadStageAccumulator>,
}

impl DownloadStageMetricsStore {
    fn start(self: &Arc<Self>, surface: &'static str) -> DownloadStageGuard {
        self.active_requests.fetch_add(1, Ordering::Relaxed);
        if surface == "diagnostic-test" {
            self.test_active_requests.fetch_add(1, Ordering::Relaxed);
        }
        DownloadStageGuard {
            store: Arc::clone(self),
            sample: Some(DownloadStageAccumulator {
                request_id: self.next_request_id.fetch_add(1, Ordering::Relaxed) + 1,
                surface,
                started: StdInstant::now(),
                started_at: OffsetDateTime::now_utc()
                    .format(&time::format_description::well_known::Rfc3339)
                    .unwrap_or_else(|_| OffsetDateTime::now_utc().unix_timestamp().to_string()),
                chunks: 0,
                client_bytes: 0,
                telegram_bytes: 0,
                telegram_retries: 0,
                first_chunk_us: None,
                telegram_us: 0,
                retry_wait_us: 0,
                decrypt_us: 0,
                verify_us: 0,
            }),
        }
    }

    fn snapshot(&self) -> DownloadStageMetrics {
        DownloadStageMetrics {
            active_requests: self.active_requests.load(Ordering::Relaxed),
            completed_requests: self.completed_requests.load(Ordering::Relaxed),
            failed_requests: self.failed_requests.load(Ordering::Relaxed),
            test_active_requests: self.test_active_requests.load(Ordering::Relaxed),
            last_test: self.last_test.lock().ok().and_then(|sample| sample.clone()),
            recent: self
                .recent
                .lock()
                .map(|samples| samples.iter().cloned().rev().collect())
                .unwrap_or_default(),
        }
    }

    fn finish(
        &self,
        sample: DownloadStageAccumulator,
        status: &'static str,
        error: Option<String>,
    ) {
        self.active_requests.fetch_sub(1, Ordering::Relaxed);
        if status == "failed" {
            self.failed_requests.fetch_add(1, Ordering::Relaxed);
        } else if status == "completed" {
            self.completed_requests.fetch_add(1, Ordering::Relaxed);
        }
        let output = DownloadStageSample {
            request_id: sample.request_id,
            surface: sample.surface.to_string(),
            started_at: sample.started_at,
            status: status.to_string(),
            chunks: sample.chunks,
            client_bytes: sample.client_bytes,
            telegram_bytes: sample.telegram_bytes,
            telegram_retries: sample.telegram_retries,
            first_chunk_us: sample.first_chunk_us,
            telegram_us: sample.telegram_us,
            retry_wait_us: sample.retry_wait_us,
            decrypt_us: sample.decrypt_us,
            verify_us: sample.verify_us,
            total_us: sample.started.elapsed().as_micros() as u64,
            error,
        };
        if sample.surface == "diagnostic-test" {
            self.test_active_requests.fetch_sub(1, Ordering::Relaxed);
            if let Ok(mut last_test) = self.last_test.lock() {
                *last_test = Some(output.clone());
            }
        }
        if let Ok(mut recent) = self.recent.lock() {
            recent.push_back(output);
            while recent.len() > 20 {
                recent.pop_front();
            }
        }
    }
}

impl DownloadStageGuard {
    fn record_chunk(&mut self, result: &ReadSpanResult) {
        let Some(sample) = self.sample.as_mut() else {
            return;
        };
        sample.chunks += 1;
        sample.telegram_bytes = sample.telegram_bytes.saturating_add(result.telegram_bytes);
        sample.telegram_retries = sample
            .telegram_retries
            .saturating_add(result.telegram_retries);
        sample.telegram_us = sample.telegram_us.saturating_add(result.telegram_us);
        sample.retry_wait_us = sample.retry_wait_us.saturating_add(result.retry_wait_us);
        sample.decrypt_us = sample.decrypt_us.saturating_add(result.decrypt_us);
        sample.verify_us = sample.verify_us.saturating_add(result.verify_us);
    }

    fn record_emitted(&mut self, bytes: u64) {
        let Some(sample) = self.sample.as_mut() else {
            return;
        };
        sample.client_bytes = sample.client_bytes.saturating_add(bytes);
        if sample.first_chunk_us.is_none() {
            sample.first_chunk_us = Some(sample.started.elapsed().as_micros() as u64);
        }
    }

    fn finish(&mut self, status: &'static str, error: Option<String>) {
        if let Some(sample) = self.sample.take() {
            self.store.finish(sample, status, error);
        }
    }
}

impl Drop for DownloadStageGuard {
    fn drop(&mut self) {
        self.finish("cancelled", None);
    }
}

#[derive(Default)]
struct TrafficCounters {
    session: TrafficCounterValues,
    total: TrafficCounterValues,
}

#[derive(Default)]
struct TrafficCounterValues {
    client_upload_bytes: AtomicU64,
    client_download_bytes: AtomicU64,
    telegram_upload_bytes: AtomicU64,
    telegram_download_bytes: AtomicU64,
}

impl TrafficCounters {
    fn from_totals(totals: TrafficTotals) -> Self {
        Self {
            session: TrafficCounterValues::default(),
            total: TrafficCounterValues {
                client_upload_bytes: AtomicU64::new(totals.client_upload_bytes),
                client_download_bytes: AtomicU64::new(totals.client_download_bytes),
                telegram_upload_bytes: AtomicU64::new(totals.telegram_upload_bytes),
                telegram_download_bytes: AtomicU64::new(totals.telegram_download_bytes),
            },
        }
    }
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
        let chunk_size = match metadata.telegram_chunk_size()? {
            Some(chunk_size) => crate::config::AppConfig::validate_chunk_size(chunk_size)?,
            None => {
                let chunk_size = config.chunk_size()?;
                metadata.set_telegram_chunk_size(chunk_size)?;
                chunk_size
            }
        };
        let download_prefetch_chunks = match metadata.telegram_download_prefetch_chunks()? {
            Some(value) => AppConfig::validate_download_prefetch_chunks(value)?,
            None => {
                let value = crate::config::DEFAULT_DOWNLOAD_PREFETCH_CHUNKS;
                metadata.set_telegram_download_prefetch_chunks(value)?;
                value
            }
        };
        let recovery_verify_interval_secs =
            match metadata.telegram_recovery_verify_interval_secs()? {
                Some(value) => AppConfig::validate_recovery_verify_interval_secs(value)?,
                None => {
                    let value = config.recovery_verify_interval_secs()?;
                    metadata.set_telegram_recovery_verify_interval_secs(value)?;
                    value
                }
            };
        let recovery_verify_enabled = match metadata.telegram_recovery_verify_enabled()? {
            Some(value) => value,
            None => {
                let value = config.recovery_verify_enabled()?;
                metadata.set_telegram_recovery_verify_enabled(value)?;
                value
            }
        };
        let recovery_verify_chunks = match metadata.telegram_recovery_verify_chunks()? {
            Some(value) => AppConfig::validate_recovery_verify_chunks(value)?,
            None => {
                let value = config.recovery_verify_chunks()?;
                metadata.set_telegram_recovery_verify_chunks(value)?;
                value
            }
        };
        let mut service = Self::new(
            metadata,
            transport_manager,
            config.data_dir(),
            chunk_size,
            download_prefetch_chunks,
            storage_chat_id,
            ObjectEncryption::from_master_key(&master_key),
        )?;
        service.config = Some(config.clone());
        service.staging_budget = config.staging_budget()?;
        service.set_recovery_verification_settings(
            recovery_verify_interval_secs,
            recovery_verify_chunks,
        )?;
        service.set_recovery_verifier_enabled(recovery_verify_enabled);
        Ok(service)
    }

    pub(crate) fn new(
        metadata: MetadataStore,
        transport_manager: std::sync::Arc<TelegramTransportManager>,
        data_dir: impl AsRef<Path>,
        chunk_size: u64,
        download_prefetch_chunks: u64,
        storage_chat_id: String,
        encryption: ObjectEncryption,
    ) -> Result<Self, ObjectFormatError> {
        let data_dir = data_dir.as_ref().to_path_buf();
        let traffic_totals = metadata.traffic_totals()?;
        fs::create_dir_all(data_dir.join(STAGING_ROOT))?;
        fs::create_dir_all(data_dir.join(MANIFEST_ROOT))?;
        fs::create_dir_all(data_dir.join(CHUNK_ROOT))?;
        fs::create_dir_all(data_dir.join(QUARANTINE_ROOT))?;
        fs::create_dir_all(data_dir.join(MULTIPART_ROOT))?;
        fs::create_dir_all(data_dir.join(CLEANUP_EVIDENCE_ROOT))?;
        Ok(Self {
            metadata: Arc::new(metadata),
            config: None,
            transport_manager,
            data_dir,
            chunk_size: Arc::new(RwLock::new(chunk_size)),
            download_prefetch_chunks: Arc::new(RwLock::new(download_prefetch_chunks)),
            recovery_verify_enabled: Arc::new(RwLock::new(
                crate::config::DEFAULT_RECOVERY_VERIFY_ENABLED,
            )),
            recovery_verify_interval_secs: Arc::new(RwLock::new(
                crate::config::DEFAULT_RECOVERY_VERIFY_INTERVAL_SECS,
            )),
            recovery_verify_chunks: Arc::new(RwLock::new(
                crate::config::DEFAULT_RECOVERY_VERIFY_CHUNKS,
            )),
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
            traffic: Arc::new(TrafficCounters::from_totals(traffic_totals)),
            download_stage_metrics: Arc::new(DownloadStageMetricsStore::default()),
            account_managers: Arc::new(Mutex::new(HashMap::new())),
        })
    }

    pub fn metadata_status(&self) -> Result<MetadataStatus, ObjectFormatError> {
        Ok(self.metadata.status()?)
    }

    pub fn durable_metrics(&self) -> Result<crate::durable::DurableMetrics, ObjectFormatError> {
        Ok(self.metadata.durable_metrics()?)
    }

    pub fn traffic_metrics(&self) -> TrafficMetrics {
        TrafficMetrics {
            session: self.traffic_snapshot(&self.traffic.session),
            total: self.traffic_snapshot(&self.traffic.total),
        }
    }

    pub fn download_stage_metrics(&self) -> DownloadStageMetrics {
        self.download_stage_metrics.snapshot()
    }

    /// Run a bounded, one-chunk diagnostic read against the first committed
    /// object. The result is recorded separately from ordinary downloads so
    /// the Overview test button never re-labels the latest client request.
    pub async fn run_download_stage_test(
        self: Arc<Self>,
    ) -> Result<DownloadStageSample, ObjectFormatError> {
        let manifest = self
            .list_manifests()?
            .into_iter()
            .filter(|manifest| {
                manifest.commit_state == crate::manifest::CommitState::Committed
                    && !manifest.chunks.is_empty()
                    && !manifest.is_expired(OffsetDateTime::now_utc())
            })
            .min_by(|left, right| {
                left.bucket
                    .cmp(&right.bucket)
                    .then_with(|| left.key.cmp(&right.key))
            })
            .ok_or_else(|| {
                ObjectFormatError::InvalidRead(
                    "no committed non-empty object is available for a stage test".into(),
                )
            })?;
        let first_chunk = manifest
            .chunks
            .first()
            .expect("filtered non-empty manifest");
        let spans = Self::plan_read(
            &manifest,
            first_chunk.offset..first_chunk.offset + first_chunk.size,
        )?
        .chunks;
        let mut stream = Box::pin(Self::read_spans_to_stream(
            Arc::clone(&self),
            &manifest,
            spans,
            "diagnostic-test",
        ));
        while let Some(result) = futures::StreamExt::next(&mut stream).await {
            result.map_err(|error| ObjectFormatError::InvalidRead(error.to_string()))?;
        }
        self.download_stage_metrics().last_test.ok_or_else(|| {
            ObjectFormatError::InvalidRead("stage test did not produce a sample".into())
        })
    }

    fn traffic_snapshot(&self, counters: &TrafficCounterValues) -> TrafficSnapshot {
        TrafficSnapshot {
            client_upload_bytes: counters.client_upload_bytes.load(Ordering::Relaxed),
            client_download_bytes: counters.client_download_bytes.load(Ordering::Relaxed),
            telegram_upload_bytes: counters.telegram_upload_bytes.load(Ordering::Relaxed),
            telegram_download_bytes: counters.telegram_download_bytes.load(Ordering::Relaxed),
        }
    }

    fn record_traffic(
        &self,
        kind: TrafficCounterKind,
        session_counter: &AtomicU64,
        total_counter: &AtomicU64,
        bytes: u64,
    ) {
        if bytes == 0 {
            return;
        }
        session_counter.fetch_add(bytes, Ordering::Relaxed);
        if let Err(error) = self.metadata.increment_traffic_total(kind, bytes) {
            warn!(
                ?error,
                counter = kind.column(),
                bytes,
                "failed to persist traffic total"
            );
            return;
        }
        total_counter.fetch_add(bytes, Ordering::Relaxed);
    }

    pub(crate) fn add_client_upload_bytes(&self, bytes: u64) {
        self.record_traffic(
            TrafficCounterKind::ClientUpload,
            &self.traffic.session.client_upload_bytes,
            &self.traffic.total.client_upload_bytes,
            bytes,
        );
    }

    fn add_client_download_bytes(&self, bytes: u64) {
        self.record_traffic(
            TrafficCounterKind::ClientDownload,
            &self.traffic.session.client_download_bytes,
            &self.traffic.total.client_download_bytes,
            bytes,
        );
    }

    fn add_telegram_upload_bytes(&self, bytes: u64) {
        self.record_traffic(
            TrafficCounterKind::TelegramUpload,
            &self.traffic.session.telegram_upload_bytes,
            &self.traffic.total.telegram_upload_bytes,
            bytes,
        );
    }

    fn add_telegram_download_bytes(&self, bytes: u64) {
        self.record_traffic(
            TrafficCounterKind::TelegramDownload,
            &self.traffic.session.telegram_download_bytes,
            &self.traffic.total.telegram_download_bytes,
            bytes,
        );
    }

    pub fn staging_budget(&self) -> u64 {
        self.staging_budget
    }

    pub fn chunk_size(&self) -> u64 {
        *self.chunk_size.read().expect("chunk size lock")
    }

    pub fn set_chunk_size(&self, chunk_size: u64) -> Result<(), ObjectFormatError> {
        crate::config::AppConfig::validate_chunk_size(chunk_size)
            .map_err(|error| ObjectFormatError::InvalidPlan(error.to_string()))?;
        *self.chunk_size.write().expect("chunk size lock") = chunk_size;
        Ok(())
    }

    pub fn download_prefetch_chunks(&self) -> u64 {
        *self
            .download_prefetch_chunks
            .read()
            .expect("download prefetch chunks lock")
    }

    pub fn set_download_prefetch_chunks(&self, chunks: u64) -> Result<(), ObjectFormatError> {
        AppConfig::validate_download_prefetch_chunks(chunks)
            .map_err(|error| ObjectFormatError::InvalidPlan(error.to_string()))?;
        *self
            .download_prefetch_chunks
            .write()
            .expect("download prefetch chunks lock") = chunks;
        Ok(())
    }

    pub fn recovery_verify_interval_secs(&self) -> u64 {
        *self
            .recovery_verify_interval_secs
            .read()
            .expect("recovery verification interval lock")
    }

    pub fn recovery_verifier_enabled(&self) -> bool {
        *self
            .recovery_verify_enabled
            .read()
            .expect("recovery verification enabled lock")
    }

    pub fn set_recovery_verifier_enabled(&self, enabled: bool) {
        *self
            .recovery_verify_enabled
            .write()
            .expect("recovery verification enabled lock") = enabled;
    }

    pub fn recovery_verify_chunks(&self) -> u64 {
        *self
            .recovery_verify_chunks
            .read()
            .expect("recovery verification chunks lock")
    }

    pub fn set_recovery_verification_settings(
        &self,
        interval_secs: u64,
        chunks: u64,
    ) -> Result<(), ObjectFormatError> {
        AppConfig::validate_recovery_verify_interval_secs(interval_secs)
            .map_err(|error| ObjectFormatError::InvalidPlan(error.to_string()))?;
        AppConfig::validate_recovery_verify_chunks(chunks)
            .map_err(|error| ObjectFormatError::InvalidPlan(error.to_string()))?;
        *self
            .recovery_verify_interval_secs
            .write()
            .expect("recovery verification interval lock") = interval_secs;
        *self
            .recovery_verify_chunks
            .write()
            .expect("recovery verification chunks lock") = chunks;
        Ok(())
    }

    /// Reach the shared SQLite store (single writer) for operator/auth tables.
    pub fn metadata_store(&self) -> &MetadataStore {
        &self.metadata
    }

    /// Return the current health of every configured Telegram account without
    /// exposing bootstrap secrets. Additional account managers are cached so
    /// this remains a cheap read on the five-second overview poll.
    pub async fn telegram_account_health_snapshots(
        &self,
    ) -> Result<Vec<TelegramAccountHealthSnapshot>, ObjectFormatError> {
        let active = self.metadata.active_connection_id()?;
        let mut snapshots = Vec::new();
        for account in self.metadata.list_telegram_accounts()? {
            let manager = if active.as_deref() == Some(account.id.as_str()) {
                Arc::clone(&self.transport_manager)
            } else {
                self.account_manager(&account.id).await?
            };
            let health = manager.health().await;
            let state = match health.state {
                TelegramConnectionState::Connected => "connected",
                TelegramConnectionState::Disconnected => "disconnected",
                TelegramConnectionState::NeedsReauth => "needs_reauth",
                TelegramConnectionState::NotConfigured => "not_configured",
            };
            snapshots.push(TelegramAccountHealthSnapshot {
                id: account.id,
                label: account.label,
                state: state.to_string(),
                detail: health.detail,
                connected: matches!(health.state, TelegramConnectionState::Connected),
                download_enabled: account.download_enabled,
            });
        }
        Ok(snapshots)
    }

    pub(crate) fn invalidate_account_manager(&self, account_id: &str) {
        if let Ok(mut managers) = self.account_managers.lock() {
            managers.remove(account_id);
        }
    }

    pub fn create_share_link(
        &self,
        manifest: &ObjectManifest,
        expires_at: Option<i64>,
        description: &str,
    ) -> Result<(String, crate::metadata::ShareLinkRecord), ObjectFormatError> {
        let token = Uuid::new_v4().to_string();
        let token_ciphertext = self.encryption.encrypt_share_token(&token)?;
        Ok(self.metadata.create_share_link_with_token(
            manifest,
            expires_at,
            description,
            &token,
            Some(&token_ciphertext),
        )?)
    }

    pub fn list_share_links(
        &self,
        bucket: &str,
        object_key: &str,
    ) -> Result<Vec<crate::metadata::ShareLinkRecord>, ObjectFormatError> {
        Ok(self.metadata.list_share_links(bucket, object_key)?)
    }

    pub fn reveal_share_token(&self, token_ciphertext: &str) -> Result<String, ObjectFormatError> {
        self.encryption.decrypt_share_token(token_ciphertext)
    }

    pub fn update_share_link_expiry(
        &self,
        id: &str,
        expires_at: Option<i64>,
    ) -> Result<Option<crate::metadata::ShareLinkRecord>, ObjectFormatError> {
        Ok(self.metadata.update_share_link_expiry(id, expires_at)?)
    }

    pub fn revoke_share_link(&self, id: &str) -> Result<bool, ObjectFormatError> {
        Ok(self.metadata.revoke_share_link_by_id(id)?)
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
        if RESERVED_BUCKET_NAMES.contains(&bucket) {
            return Err(ObjectFormatError::InvalidBucketName(format!(
                "{bucket} is reserved for an internal HTTP route"
            )));
        }
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
        let now = OffsetDateTime::now_utc();
        for manifest in self.metadata.list_bucket_manifests(bucket, None)? {
            if manifest.is_expired(now) {
                self.metadata
                    .tombstone_manifest(manifest.object_id, "object expired")?;
            }
        }
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
        let manifest = self
            .metadata
            .get_active_manifest(bucket, key)?
            .filter(|manifest| !manifest.is_expired(OffsetDateTime::now_utc()));
        if let Some(manifest) = manifest {
            if self.metadata.object_rechunk_locked(manifest.object_id)? {
                return Err(ObjectFormatError::InvalidRead(
                    "object is being re-chunked; try again later".to_string(),
                ));
            }
            return Ok(Some(manifest));
        }
        Ok(None)
    }

    pub fn list_bucket_manifests(
        &self,
        bucket: &str,
        prefix: Option<&str>,
    ) -> Result<Vec<ObjectManifest>, ObjectFormatError> {
        Ok(self
            .metadata
            .list_bucket_manifests(bucket, prefix)?
            .into_iter()
            .filter(|manifest| !manifest.is_expired(OffsetDateTime::now_utc()))
            .collect())
    }

    pub fn get_manifest(
        &self,
        object_id: Uuid,
    ) -> Result<Option<ObjectManifest>, ObjectFormatError> {
        Ok(self.metadata.get_manifest(object_id)?)
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

    pub fn delete_empty_folder(
        &self,
        bucket: &str,
        folder_key: &str,
    ) -> Result<Option<ObjectManifest>, ObjectFormatError> {
        Ok(self
            .metadata
            .delete_empty_folder(bucket, folder_key, "deleted via admin")?)
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
        self.initiate_multipart_upload_with_expiry(
            bucket,
            key,
            content_type,
            checksum_algorithm,
            None,
        )
    }

    pub fn initiate_multipart_upload_with_expiry(
        &self,
        bucket: &str,
        key: &str,
        content_type: &str,
        checksum_algorithm: Option<&str>,
        expires_at: Option<OffsetDateTime>,
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
            expires_at,
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
            MultipartState::Completing
                | MultipartState::Aborted
                | MultipartState::Completed
                | MultipartState::Quarantined
                | MultipartState::RecoveryRequired
        ) {
            return Err(ObjectFormatError::InvalidPlan(format!(
                "multipart upload is not active: {upload_id}"
            )));
        }

        if let Some(existing) = self
            .metadata
            .active_multipart_part_transfer(upload_id, part_number)?
        {
            // Dokploy retries UploadPart after a timed-out response. Reuse the
            // earliest durable job for that part rather than staging and
            // publishing a second copy to Telegram.
            self.discard_duplicate_body(body).await;
            if existing.state == "recovery_required" {
                return Err(ObjectFormatError::InvalidPlan(existing.error.unwrap_or_else(
                    || {
                        "a prior multipart part needs Telegram reconciliation before it can be retried"
                            .into()
                    },
                )));
            }
            self.wait_transfer(&existing.id).await?;
            return self
                .metadata
                .get_multipart_part(upload_id, part_number)?
                .ok_or_else(|| {
                    ObjectFormatError::InvalidPlan(
                        "reused multipart transfer completed without a part manifest".into(),
                    )
                });
        }

        let job = self
            .enqueue_with_part(
                &session.bucket,
                &session.key,
                &session.content_type,
                body,
                Some((upload_id, part_number, checksum.map(str::to_string))),
                None,
                session.expires_at,
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
        if session.state == MultipartState::Completing {
            let job_id = self
                .metadata
                .multipart_completion_job(plan.upload_id)?
                .ok_or_else(|| {
                    ObjectFormatError::InvalidPlan(
                        "multipart completion job is missing; recovery is required".into(),
                    )
                })?;
            return self.wait_transfer(&job_id).await;
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

        if plan.object_id != plan.upload_id {
            return Err(ObjectFormatError::InvalidPlan(
                "multipart completion object id must match the upload id".to_string(),
            ));
        }
        if plan.checksum_algorithm != session.checksum_algorithm {
            return Err(ObjectFormatError::InvalidPlan(
                "multipart checksum algorithm changed during completion".to_string(),
            ));
        }

        let mut chunks = Vec::new();
        let mut offset = 0_u64;
        let mut previous_part_number = 0_u32;
        let mut encryption = None;
        let mut composite_hasher = Sha256::new();
        composite_hasher.update(b"telegram-s3-multipart-sha256-v1\0");
        for part_plan in &plan.parts {
            if part_plan.part_number <= previous_part_number {
                return Err(ObjectFormatError::InvalidPlan(
                    "multipart parts must be unique and ordered by part number".to_string(),
                ));
            }
            previous_part_number = part_plan.part_number;
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
            manifest
                .validate()
                .map_err(ObjectFormatError::InvalidPlan)?;
            if stored.size != part_plan.size
                || manifest.content_length != stored.size
                || stored.checksum != part_plan.checksum
                || manifest.checksum.whole_object != stored.checksum
            {
                return Err(ObjectFormatError::InvalidPlan(
                    "multipart part metadata changed during completion".to_string(),
                ));
            }
            if let Some(expected) = &encryption {
                if expected != &manifest.encryption {
                    return Err(ObjectFormatError::InvalidPlan(
                        "multipart parts use incompatible encryption metadata".to_string(),
                    ));
                }
            } else {
                encryption = Some(manifest.encryption.clone());
            }

            composite_hasher.update(part_plan.part_number.to_be_bytes());
            composite_hasher.update(stored.size.to_be_bytes());
            update_checksum_component(&mut composite_hasher, &manifest.checksum.algorithm);
            update_checksum_component(&mut composite_hasher, &manifest.checksum.whole_object);

            for source_chunk in &manifest.chunks {
                if source_chunk.telegram_peer_id.trim().is_empty()
                    || source_chunk.telegram_message_id <= 0
                {
                    return Err(ObjectFormatError::InvalidPlan(format!(
                        "multipart part {} has an unpublished chunk",
                        part_plan.part_number
                    )));
                }
                let order = u32::try_from(chunks.len()).map_err(|_| {
                    ObjectFormatError::InvalidPlan(
                        "multipart object contains too many chunks".to_string(),
                    )
                })?;
                let (source_object_id, source_chunk_order) =
                    source_chunk.payload_identity(manifest.object_id);
                chunks.push(ChunkRef {
                    order,
                    offset,
                    size: source_chunk.size,
                    checksum: source_chunk.checksum.clone(),
                    telegram_peer_id: source_chunk.telegram_peer_id.clone(),
                    telegram_message_id: source_chunk.telegram_message_id,
                    telegram_document_id: source_chunk.telegram_document_id.clone(),
                    source_object_id: Some(source_object_id),
                    source_chunk_order: Some(source_chunk_order),
                    replicas: source_chunk.replicas.clone(),
                });
                offset = offset.checked_add(source_chunk.size).ok_or_else(|| {
                    ObjectFormatError::InvalidPlan("multipart content length overflow".to_string())
                })?;
            }
        }

        if offset != plan.content_length {
            return Err(ObjectFormatError::InvalidPlan(format!(
                "multipart content length mismatch: expected {}, composed {}",
                plan.content_length, offset
            )));
        }

        let manifest = ObjectManifest {
            schema_version: MANIFEST_SCHEMA_VERSION,
            commit_state: CommitState::Staging,
            object_id: plan.object_id,
            bucket: plan.bucket.clone(),
            key: plan.key.clone(),
            version_id: Some(plan.object_id.to_string()),
            content_length: offset,
            content_type: plan.content_type.clone(),
            user_metadata: BTreeMap::new(),
            tags: BTreeMap::new(),
            created_at: OffsetDateTime::now_utc(),
            expires_at: session.expires_at,
            checksum: ObjectChecksum {
                algorithm: MULTIPART_CHECKSUM_ALGORITHM.to_string(),
                whole_object: hex::encode(composite_hasher.finalize()),
            },
            encryption: encryption.ok_or_else(|| {
                ObjectFormatError::InvalidPlan(
                    "multipart completion has no encryption metadata".to_string(),
                )
            })?,
            telegram: TelegramLocation {
                peer_id: self.storage_chat_id()?,
                message_id: 0,
                document_id: Some(format!("local:{}:manifest", plan.object_id)),
            },
            chunks,
        };
        manifest
            .validate()
            .map_err(ObjectFormatError::InvalidPlan)?;

        let job_id = self
            .metadata
            .begin_transfer(plan.object_id, &plan.bucket, &plan.key)?;
        let staging_dir = self.staging_dir(plan.object_id);
        let queued = (|| -> Result<crate::durable::TransferJob, ObjectFormatError> {
            fs::create_dir_all(&staging_dir)?;
            let manifest_bytes = serde_json::to_vec_pretty(&manifest)?;
            self.metadata.reserve_staging(
                &job_id,
                manifest_bytes.len() as u64,
                self.staging_budget,
            )?;
            write_json_file(&staging_dir.join(MANIFEST_FILE_NAME), &manifest)?;
            let operation = self
                .metadata
                .stage_manifest(OperationKind::Put, manifest.clone())?;
            self.metadata
                .queue_multipart_completion(&job_id, operation, plan.upload_id)?;
            self.metadata.transfer(&job_id)?.ok_or_else(|| {
                ObjectFormatError::InvalidPlan("queued multipart completion missing".into())
            })
        })();
        let job = match queued {
            Ok(job) => job,
            Err(error) => {
                let removed = match fs::remove_dir_all(&staging_dir) {
                    Ok(()) => true,
                    Err(io_error) if io_error.kind() == io::ErrorKind::NotFound => true,
                    Err(_) => false,
                };
                let _ = self.metadata.fail_reception(
                    &job_id,
                    removed,
                    "Multipart completion could not be durably queued",
                );
                return Err(error);
            }
        };
        self.ensure_workers();
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
        Ok(manifest.filter(|manifest| {
            manifest.bucket == bucket
                && manifest.key == key
                && !manifest.is_expired(OffsetDateTime::now_utc())
        }))
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
        let telegram_files_bytes = telegram_files_bytes(&manifests);

        Ok(ObjectFormatStatus {
            data_dir: self.data_dir.clone(),
            chunk_size: self.chunk_size(),
            committed_objects,
            staged_objects,
            recovery_required_objects,
            orphaned_chunks,
            telegram_files_bytes,
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
                CommitState::Committed => {
                    let mut sampled_issues = self.inspect_committed_manifest(&manifest).await?;
                    let durable_issues: Vec<RecoveryIssue> = sampled_issues
                        .iter()
                        .filter(|issue| {
                            matches!(
                                issue.kind.as_str(),
                                "invalid_manifest" | "missing_chunk" | "corrupted_chunk"
                            )
                        })
                        .cloned()
                        .collect();
                    if !durable_issues.is_empty() {
                        self.metadata.update_manifest_state(
                            manifest.object_id,
                            CommitState::RecoveryRequired,
                        )?;
                        let mut persisted = durable_issues.clone();
                        for issue in &mut persisted {
                            issue.commit_state = Some(CommitState::RecoveryRequired);
                        }
                        self.metadata.set_object_recovery_marker(
                            manifest.object_id,
                            &manifest.bucket,
                            &manifest.key,
                            &serde_json::to_string(&persisted)?,
                        )?;
                        for issue in &mut sampled_issues {
                            issue.commit_state = Some(CommitState::RecoveryRequired);
                        }
                    }
                    issues.extend(sampled_issues);
                }
                CommitState::RecoveryRequired => {
                    if let Some(details_json) =
                        self.metadata.object_recovery_marker(manifest.object_id)?
                    {
                        let persisted = serde_json::from_str::<Vec<RecoveryIssue>>(&details_json)?;
                        issues.extend(persisted);
                    } else {
                        issues.push(self.build_recovery_issue(
                            &manifest,
                            "object_recovery_required",
                            "object requires recovery".to_string(),
                            vec![
                                "the object is hidden from S3 reads until a full repair verification succeeds".to_string(),
                                "re-upload or restore the original source; the server cannot recreate a missing chunk from metadata alone".to_string(),
                            ],
                        ));
                    }
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
        Self::plan_chunks(content_length, self.chunk_size())
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

        let chunk_size = self.chunk_size();
        let mut chunk_plan = ChunkPlan {
            chunk_size,
            content_length: 0,
            chunks: Vec::new(),
        };
        let mut chunk_refs = Vec::new();
        let mut whole_hasher = Sha256::new();
        let mut buffer = vec![0_u8; chunk_size as usize];
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
                source_object_id: None,
                source_chunk_order: None,
                replicas: Vec::new(),
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
            expires_at: None,
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
        self.put_stream_with_expiry(bucket, key, content_type, body, conditionals, None)
            .await
    }

    pub async fn put_stream_with_expiry(
        &self,
        bucket: &str,
        key: &str,
        content_type: &str,
        body: Option<StreamingBlob>,
        conditionals: Option<crate::durable::TransferWriteConditionals>,
        expires_at: Option<OffsetDateTime>,
    ) -> Result<ObjectManifest, ObjectFormatError> {
        let job = self
            .enqueue_stream_with_expiry(bucket, key, content_type, body, conditionals, expires_at)
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
        if manifest.is_expired(OffsetDateTime::now_utc()) {
            return Err(ObjectFormatError::InvalidRead(format!(
                "object not found: {bucket}/{key}"
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
                let (source_object_id, source_order) = chunk.payload_identity(manifest.object_id);
                self.decrypt_chunk(source_object_id, source_order, &ciphertext)?
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
    /// the chunk size and configured prefetch window, and never allocating a
    /// whole object in memory. The first requested span is fetched on its own;
    /// speculative prefetch starts only after that span is ready for the client.
    ///
    /// This is the single shared streaming reader used by both the S3
    /// `get_object` path and the `/_admin` download endpoint so their byte
    /// output stays identical. Errors surface per-chunk as stream items so a
    /// failure mid-download aborts the stream instead of buffering.
    pub fn read_spans_to_stream(
        this: Arc<Self>,
        manifest: &ObjectManifest,
        spans: Vec<ReadSpan>,
        surface: &'static str,
    ) -> impl futures::Stream<Item = Result<Bytes, io::Error>> + Send + 'static + use<> {
        let pin = this.pin_object(manifest.object_id);
        let stage = this.download_stage_metrics.start(surface);
        let concurrency = usize::try_from(this.download_prefetch_chunks().saturating_add(1))
            .unwrap_or(usize::MAX)
            .max(1);
        let mut spans = spans.into_iter();
        let first_span = spans.next();
        let remaining_spans = spans.collect::<Vec<_>>();
        let first = futures::stream::iter(first_span)
            .map({
                let object_format = Arc::clone(&this);
                let manifest = manifest.clone();
                move |span| read_stream_span(Arc::clone(&object_format), manifest.clone(), span)
            })
            .buffered(1);
        let prefetched = futures::stream::iter(remaining_spans)
            .map({
                let object_format = Arc::clone(&this);
                let manifest = manifest.clone();
                move |span| read_stream_span(Arc::clone(&object_format), manifest.clone(), span)
            })
            .buffered(concurrency);
        let buffered = first.chain(prefetched);
        let buffered = Box::pin(buffered);
        futures::stream::unfold(
            (buffered, this, pin, stage, false),
            |(mut buffered, object_format, pin, mut stage, done)| async move {
                if done {
                    stage.finish("cancelled", None);
                    return None;
                }
                match buffered.as_mut().next().await {
                    Some(Ok(result)) => {
                        let length = result.length;
                        stage.record_chunk(&result);
                        stage.record_emitted(length);
                        let bytes = result.bytes;
                        Some((
                            Ok({
                                object_format.add_client_download_bytes(length);
                                bytes
                            }),
                            (buffered, object_format, pin, stage, false),
                        ))
                    }
                    Some(Err(error)) => {
                        stage.finish("failed", Some(error.to_string()));
                        Some((Err(error), (buffered, object_format, pin, stage, true)))
                    }
                    None => {
                        stage.finish("completed", None);
                        None
                    }
                }
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
                let (source_object_id, source_order) = chunk.payload_identity(manifest.object_id);
                self.decrypt_chunk(source_object_id, source_order, &bytes)?
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
            if chunk.references_remote_payload() {
                if chunk.telegram_peer_id.trim().is_empty() || chunk.telegram_message_id <= 0 {
                    return Ok(false);
                }
                continue;
            }
            let path = staging_dir.join(chunk_file_name(chunk.order));
            if !path.exists() {
                return Ok(false);
            }
            let mut file = File::open(&path)?;
            let mut bytes = Vec::new();
            file.read_to_end(&mut bytes)?;
            let plaintext = if manifest.encryption.enabled {
                let (source_object_id, source_order) = chunk.payload_identity(manifest.object_id);
                self.decrypt_chunk(source_object_id, source_order, &bytes)?
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
            if chunk.references_remote_payload() {
                continue;
            }
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
                let (source_object_id, source_order) = chunk.payload_identity(manifest.object_id);
                match self.decrypt_chunk(source_object_id, source_order, &bytes) {
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
        let mut unavailable_details = Vec::new();
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

        let chunk_indices =
            random_chunk_sample(manifest.chunks.len(), self.recovery_verify_chunks());
        for index in chunk_indices {
            let chunk = &manifest.chunks[index];
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
                        unavailable_details.push(format!(
                            "chunk {} could not be verified from Telegram message {}: {}",
                            chunk.order, message_id, message
                        ));
                    }
                    continue;
                }
            };

            let plaintext = if manifest.encryption.enabled {
                let (source_object_id, source_order) = chunk.payload_identity(manifest.object_id);
                match self.decrypt_chunk(source_object_id, source_order, &ciphertext) {
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
        if !unavailable_details.is_empty() {
            issues.push(self.build_recovery_issue(
                manifest,
                "verification_unavailable",
                "one or more sampled chunks could not be verified yet".to_string(),
                unavailable_details,
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
            schema_version: MANIFEST_SCHEMA_VERSION,
            commit_state: args.commit_state,
            object_id: args.object_id,
            bucket: args.bucket,
            key: args.key,
            version_id: Some(args.object_id.to_string()),
            content_length: args.chunks.iter().map(|chunk| chunk.size).sum(),
            content_type: args.content_type,
            expires_at: args.expires_at,
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
            // The mock runtime is also the deterministic fault-injection
            // backend used by release tests. It is intentionally controlled
            // only through an explicit test-prefixed environment variable and
            // is never consulted by a live Telegram transport.
            let fault = std::env::var("TELEGRAM_MOCK_FAULT").ok();
            if let Some(fault) = fault.as_deref() {
                let message = match fault {
                    "peer_lookup" => "storage peer lookup failed: scripted test fault",
                    "timeout" => "request timeout: scripted test fault",
                    "proxy_disconnect" => "proxy disconnected: scripted test fault",
                    "flood_wait" => "FLOOD_WAIT_1: scripted test fault",
                    "auth_key_unregistered" => "AUTH_KEY_UNREGISTERED: scripted test fault",
                    "missing" => "remote document was not acknowledged: scripted test fault",
                    _ => "",
                };
                if !message.is_empty() {
                    return Err(ObjectFormatError::Telegram(
                        crate::telegram::TelegramTransportError::Rpc(message.to_string()),
                    ));
                }
            }
            let message_id = self.next_mock_message_id()?;
            let mock_dir = self.mock_telegram_dir();
            fs::create_dir_all(&mock_dir)?;
            let destination = mock_dir.join(format!("{message_id}.bin"));
            fs::copy(path, &destination)?;
            self.add_telegram_upload_bytes(fs::metadata(path)?.len());
            fs::write(
                mock_dir.join(format!("{message_id}.json")),
                serde_json::to_vec(&serde_json::json!({ "file_name": file_name }))?,
            )?;
            if fault.as_deref() == Some("ambiguous") {
                return Err(ObjectFormatError::Telegram(
                    crate::telegram::TelegramTransportError::Rpc(
                        "request timeout after remote send: scripted ambiguous acknowledgement"
                            .to_string(),
                    ),
                ));
            }
            if fault.as_deref() == Some("byte_collision") {
                fs::write(&destination, b"different encrypted bytes")?;
                return Err(ObjectFormatError::Telegram(
                    crate::telegram::TelegramTransportError::Rpc(
                        "request timeout after remote send: scripted byte collision".to_string(),
                    ),
                ));
            }
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
        self.add_telegram_upload_bytes(size);
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
            return self.download_message_bytes_once(message_id).await;
        }
        retry_telegram_read(transport.retry_policy(), message_id, || {
            self.download_message_bytes_once(message_id)
        })
        .await
    }

    async fn download_message_bytes_for_stream(
        &self,
        message_id: i32,
    ) -> Result<TelegramStreamRead, ObjectFormatError> {
        let transport = self.transport_manager.current().await?;
        self.download_message_bytes_for_stream_from_transport(transport, message_id)
            .await
    }

    async fn download_message_bytes_for_stream_from_transport(
        &self,
        transport: Arc<crate::telegram::TelegramTransport>,
        message_id: i32,
    ) -> Result<TelegramStreamRead, ObjectFormatError> {
        let started = StdInstant::now();
        if transport.is_mock() {
            let bytes = self
                .download_message_bytes_once_with_transport(transport, message_id)
                .await?;
            return Ok(TelegramStreamRead {
                telegram_us: started.elapsed().as_micros() as u64,
                bytes,
                retries: 0,
                retry_wait_us: 0,
            });
        }
        let retry_policy = transport.retry_policy();
        retry_telegram_read_for_stream(
            retry_policy,
            message_id,
            TELEGRAM_STREAM_RECOVERY_WINDOW,
            || self.download_message_bytes_once_with_transport(Arc::clone(&transport), message_id),
        )
        .await
        .map(|result| TelegramStreamRead {
            telegram_us: started.elapsed().as_micros() as u64,
            bytes: result.bytes,
            retries: result.retries,
            retry_wait_us: result.retry_wait_us,
        })
    }

    async fn download_message_bytes_once(
        &self,
        message_id: i32,
    ) -> Result<Vec<u8>, ObjectFormatError> {
        let transport = self.transport_manager.current().await?;
        self.download_message_bytes_once_with_transport(transport, message_id)
            .await
    }

    async fn download_message_bytes_once_with_transport(
        &self,
        transport: Arc<crate::telegram::TelegramTransport>,
        message_id: i32,
    ) -> Result<Vec<u8>, ObjectFormatError> {
        if transport.is_mock() {
            if let Ok(fault) = std::env::var("TELEGRAM_MOCK_READ_FAULT") {
                return Err(ObjectFormatError::Telegram(TelegramTransportError::Rpc(
                    format!("{fault}: scripted mock read fault for message {message_id}"),
                )));
            }
            let path = self.mock_telegram_dir().join(format!("{message_id}.bin"));
            if !path.exists() {
                return Err(ObjectFormatError::InvalidRead(format!(
                    "telegram message not found: {message_id}"
                )));
            }
            let bytes = fs::read(path)?;
            self.add_telegram_download_bytes(bytes.len() as u64);
            return Ok(bytes);
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
            self.add_telegram_download_bytes(chunk.len() as u64);
            bytes.extend_from_slice(&chunk);
        }
        Ok(bytes)
    }

    fn is_retryable_telegram_read_error(error: &ObjectFormatError) -> bool {
        matches!(
            error,
            ObjectFormatError::Telegram(TelegramTransportError::Rpc(_))
                | ObjectFormatError::Telegram(TelegramTransportError::Io(_))
                | ObjectFormatError::Telegram(TelegramTransportError::Proxy(
                    crate::telegram::ProxyError::BridgeFailed(_),
                ))
        )
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

async fn retry_telegram_read<F, Fut>(
    retry_policy: RetryPolicy,
    message_id: i32,
    mut read: F,
) -> Result<Vec<u8>, ObjectFormatError>
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = Result<Vec<u8>, ObjectFormatError>>,
{
    let mut attempt = 1;
    loop {
        match read().await {
            Ok(bytes) => return Ok(bytes),
            Err(error) => {
                if !ObjectFormatService::is_retryable_telegram_read_error(&error) {
                    return Err(error);
                }
                let error_text = error.to_string();
                let decision =
                    retry_policy.retry_decision(attempt, parse_flood_wait_seconds(&error_text));
                let delay = match decision {
                    RetryDecision::RetryAfter(delay) | RetryDecision::RespectFloodWait(delay) => {
                        delay
                    }
                    RetryDecision::GiveUp => return Err(error),
                };
                warn!(
                    telegram_message_id = message_id,
                    attempt,
                    retry_delay_ms = delay.as_millis() as u64,
                    error = %error_text,
                    "retrying Telegram chunk download"
                );
                tokio::time::sleep(delay).await;
                attempt += 1;
            }
        }
    }
}

async fn retry_telegram_read_for_stream<F, Fut>(
    retry_policy: RetryPolicy,
    message_id: i32,
    recovery_window: StdDuration,
    mut read: F,
) -> Result<TelegramRetryResult, ObjectFormatError>
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = Result<Vec<u8>, ObjectFormatError>>,
{
    let deadline = StdInstant::now() + recovery_window;
    let mut attempt = 1_u32;
    let mut retries = 0_u64;
    let mut retry_wait_us = 0_u64;
    loop {
        let remaining = deadline.saturating_duration_since(StdInstant::now());
        if remaining.is_zero() {
            return Err(ObjectFormatError::Telegram(TelegramTransportError::Io(
                io::Error::new(
                    io::ErrorKind::TimedOut,
                    format!(
                        "Telegram chunk download recovery window expired for message {message_id}"
                    ),
                ),
            )));
        }
        let result = match tokio::time::timeout(remaining, read()).await {
            Ok(result) => result,
            Err(_) => {
                return Err(ObjectFormatError::Telegram(TelegramTransportError::Io(
                    io::Error::new(
                        io::ErrorKind::TimedOut,
                        format!(
                            "Telegram chunk download recovery window expired for message {message_id}"
                        ),
                    ),
                )));
            }
        };
        match result {
            Ok(bytes) => {
                return Ok(TelegramRetryResult {
                    bytes,
                    retries,
                    retry_wait_us,
                });
            }
            Err(error) => {
                if !ObjectFormatService::is_retryable_telegram_read_error(&error) {
                    return Err(error);
                }
                let error_text = error.to_string();
                let delay = if let Some(seconds) = parse_flood_wait_seconds(&error_text)
                    && retry_policy.respect_flood_wait
                {
                    StdDuration::from_secs(seconds.max(1))
                } else {
                    retry_policy.backoff_for_attempt(attempt.saturating_add(1))
                };
                let remaining = deadline.saturating_duration_since(StdInstant::now());
                if remaining.is_zero() {
                    return Err(error);
                }
                let wait = delay.min(remaining);
                warn!(
                    telegram_message_id = message_id,
                    attempt,
                    retry_delay_ms = wait.as_millis() as u64,
                    recovery_window_secs = TELEGRAM_STREAM_RECOVERY_WINDOW_SECS,
                    error = %error_text,
                    "holding Telegram-backed stream open while retrying chunk download"
                );
                tokio::time::sleep(wait).await;
                retries = retries.saturating_add(1);
                retry_wait_us = retry_wait_us.saturating_add(wait.as_micros() as u64);
                if wait >= remaining {
                    return Err(error);
                }
                attempt = attempt.saturating_add(1);
            }
        }
    }
}

struct TelegramRetryResult {
    bytes: Vec<u8>,
    retries: u64,
    retry_wait_us: u64,
}

struct TelegramStreamRead {
    bytes: Vec<u8>,
    retries: u64,
    retry_wait_us: u64,
    telegram_us: u64,
}

async fn read_stream_span(
    object_format: Arc<ObjectFormatService>,
    manifest: ObjectManifest,
    span: ReadSpan,
) -> Result<ReadSpanResult, io::Error> {
    let chunk = manifest.chunks.get(span.order as usize).ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::NotFound,
            format!("missing chunk {}", span.order),
        )
    })?;
    // Rotate deterministically across the primary location and replicas, but
    // only among accounts explicitly enabled for downloads. Ownership remains
    // untouched: this flag affects reads only, never upload or cleanup jobs.
    let primary_account = object_format
        .metadata
        .active_connection_id()
        .map_err(|error| io::Error::other(error.to_string()))?;
    let primary_enabled = match primary_account.as_deref() {
        Some(account_id) => object_format
            .metadata
            .telegram_account_download_enabled(account_id)
            .map_err(|error| io::Error::other(error.to_string()))?,
        None => true,
    };
    let enabled_replicas = chunk
        .replicas
        .iter()
        .filter(|replica| {
            object_format
                .metadata
                .telegram_account_download_enabled(&replica.account_id)
                .unwrap_or(false)
        })
        .collect::<Vec<_>>();
    let candidate_count = usize::from(primary_enabled) + enabled_replicas.len();
    if candidate_count == 0 {
        return Err(io::Error::new(
            io::ErrorKind::NotFound,
            "no Telegram account enabled for downloading this object",
        ));
    }
    let selected = (span.order as usize) % candidate_count;
    let (location, selected_transport) = if primary_enabled && selected == 0 {
        (
            TelegramLocation {
                peer_id: chunk.telegram_peer_id.clone(),
                message_id: chunk.telegram_message_id,
                document_id: chunk.telegram_document_id.clone(),
            },
            None,
        )
    } else {
        let replica_index = selected.saturating_sub(usize::from(primary_enabled));
        let replica = enabled_replicas[replica_index];
        let manager = object_format
            .account_manager(&replica.account_id)
            .await
            .map_err(|error| io::Error::other(error.to_string()))?;
        let transport = manager
            .current()
            .await
            .map_err(|error| io::Error::other(error.to_string()))?;
        (
            TelegramLocation {
                peer_id: replica.telegram_peer_id.clone(),
                message_id: replica.telegram_message_id,
                document_id: replica.telegram_document_id.clone(),
            },
            Some(transport),
        )
    };
    let message_id = i32::try_from(location.message_id).map_err(|_| {
        io::Error::new(
            io::ErrorKind::InvalidData,
            format!("telegram message id out of range for chunk {}", span.order),
        )
    })?;
    let telegram = match selected_transport {
        Some(transport) => {
            object_format
                .download_message_bytes_for_stream_from_transport(transport, message_id)
                .await
        }
        None => {
            object_format
                .download_message_bytes_for_stream(message_id)
                .await
        }
    }
    .map_err(|error| {
        warn!(
            object_id = %manifest.object_id,
            chunk_order = span.order,
            telegram_message_id = message_id,
            error = %error,
            "object stream chunk failed"
        );
        io::Error::other(error.to_string())
    })?;
    let telegram_bytes = telegram.bytes.len() as u64;
    let telegram_retries = telegram.retries;
    let telegram_us = telegram.telegram_us;
    let retry_wait_us = telegram.retry_wait_us;
    let decrypt_started = StdInstant::now();
    let plaintext = if manifest.encryption.enabled {
        let (source_object_id, source_order) = chunk.payload_identity(manifest.object_id);
        object_format
            .decrypt_chunk(source_object_id, source_order, &telegram.bytes)
            .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error.to_string()))?
    } else {
        telegram.bytes
    };
    let decrypt_us = decrypt_started.elapsed().as_micros() as u64;
    let verify_started = StdInstant::now();
    let actual_checksum = sha256_hex(&plaintext);
    if actual_checksum != span.checksum {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!(
                "checksum mismatch for chunk {}: expected {}, got {}",
                span.order, span.checksum, actual_checksum
            ),
        ));
    }
    let verify_us = verify_started.elapsed().as_micros() as u64;
    let start = span.offset_within_chunk as usize;
    let end = start + span.length as usize;
    if plaintext.len() < end {
        return Err(io::Error::new(
            io::ErrorKind::UnexpectedEof,
            format!(
                "chunk {} shorter than planned span (len {}, need {}-{})",
                span.order,
                plaintext.len(),
                start,
                end
            ),
        ));
    }
    Ok(ReadSpanResult {
        length: span.length,
        bytes: Bytes::copy_from_slice(&plaintext[start..end]),
        telegram_bytes,
        telegram_retries,
        telegram_us,
        retry_wait_us,
        decrypt_us,
        verify_us,
    })
}

struct ReadSpanResult {
    length: u64,
    bytes: Bytes,
    telegram_bytes: u64,
    telegram_retries: u64,
    telegram_us: u64,
    retry_wait_us: u64,
    decrypt_us: u64,
    verify_us: u64,
}

fn verify_staged_chunks(
    staging_dir: &Path,
    manifest: &ObjectManifest,
    encryption: &ObjectEncryption,
) -> Result<(), ObjectFormatError> {
    for chunk in &manifest.chunks {
        if chunk.references_remote_payload() {
            if chunk.telegram_peer_id.trim().is_empty() || chunk.telegram_message_id <= 0 {
                return Err(ObjectFormatError::InvalidPlan(format!(
                    "composed chunk {} has no durable Telegram location",
                    chunk.order
                )));
            }
            continue;
        }
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
            let (source_object_id, source_order) = chunk.payload_identity(manifest.object_id);
            encryption.decrypt_chunk(source_object_id, source_order, &bytes)?
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

fn update_checksum_component(hasher: &mut Sha256, value: &str) {
    hasher.update((value.len() as u64).to_be_bytes());
    hasher.update(value.as_bytes());
}

pub fn parse_checksum_hex(value: &str) -> Result<Vec<u8>, ObjectFormatError> {
    hex::decode(value).map_err(|error| ObjectFormatError::InvalidChecksum(error.to_string()))
}

fn chunk_file_name(order: u32) -> String {
    format!("chunk-{order:08}.bin")
}

/// Select a uniformly random set of distinct chunk indexes without allocating
/// a second list the size of a large object. A fresh OS-random source is used
/// on every scan, so the verifier does not repeatedly inspect the same prefix.
fn random_chunk_sample(chunk_count: usize, requested: u64) -> Vec<usize> {
    let requested = usize::try_from(requested)
        .unwrap_or(usize::MAX)
        .min(chunk_count);
    if requested == 0 || chunk_count == 0 {
        return Vec::new();
    }

    let mut rng = OsRng;
    let mut selected = Vec::with_capacity(requested);
    for index in 0..chunk_count {
        if selected.len() < requested {
            selected.push(index);
            continue;
        }
        let slot = random_below(&mut rng, index + 1);
        if slot < requested {
            selected[slot] = index;
        }
    }
    selected
}

fn random_below(rng: &mut OsRng, upper: usize) -> usize {
    debug_assert!(upper > 0);
    let upper = u64::try_from(upper).expect("usize must fit in u64");
    let threshold = upper.wrapping_neg() % upper;
    loop {
        let value = rng.next_u64();
        if value >= threshold {
            return (value % upper) as usize;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::AppConfig;
    use crate::manifest::CommittedManifestArgs;
    use crate::metadata::TelegramBootstrapSettings;
    use crate::multipart::{MultipartCompletionPlan, MultipartPartPlan};
    use crate::object_format::workflow::RemoteReconciliation;
    use crate::telegram::{
        TelegramConnectionHealth, TelegramConnectionState, TelegramTransportManager,
    };
    use bytes::Bytes;
    use std::env;
    use std::sync::atomic::{AtomicUsize, Ordering};
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
    fn internal_http_route_bucket_names_are_reserved() {
        assert_eq!(RESERVED_BUCKET_NAMES, ["_public", "_admin"]);
        assert!(RESERVED_BUCKET_NAMES.contains(&"_public"));
        assert!(RESERVED_BUCKET_NAMES.contains(&"_admin"));
        assert!(!RESERVED_BUCKET_NAMES.contains(&"documents"));
    }

    #[tokio::test]
    async fn create_bucket_rejects_internal_http_route_names() {
        let tempdir = TempDir::new().expect("tempdir");
        let service = sample_service(&tempdir).await;
        for name in RESERVED_BUCKET_NAMES {
            assert!(matches!(
                service.create_bucket(name),
                Err(ObjectFormatError::InvalidBucketName(_))
            ));
        }
    }

    #[tokio::test]
    async fn transient_telegram_read_is_retried_before_returning_data() {
        let attempts = std::sync::Arc::new(AtomicUsize::new(0));
        let result =
            retry_telegram_read(RetryPolicy::new(3, std::time::Duration::ZERO, true), 17, {
                let attempts = std::sync::Arc::clone(&attempts);
                move || {
                    let attempt = attempts.fetch_add(1, Ordering::SeqCst);
                    async move {
                        if attempt == 0 {
                            Err(ObjectFormatError::Telegram(TelegramTransportError::Rpc(
                                "temporary timeout".to_string(),
                            )))
                        } else {
                            Ok(vec![1, 2, 3])
                        }
                    }
                }
            })
            .await
            .expect("transient read should recover");

        assert_eq!(result, vec![1, 2, 3]);
        assert_eq!(attempts.load(Ordering::SeqCst), 2);
    }

    #[tokio::test]
    async fn stream_read_can_retry_beyond_the_normal_attempt_limit() {
        let attempts = std::sync::Arc::new(AtomicUsize::new(0));
        let result = retry_telegram_read_for_stream(
            RetryPolicy::new(1, std::time::Duration::ZERO, true),
            19,
            std::time::Duration::from_secs(1),
            {
                let attempts = std::sync::Arc::clone(&attempts);
                move || {
                    let attempt = attempts.fetch_add(1, Ordering::SeqCst);
                    async move {
                        if attempt < 5 {
                            Err(ObjectFormatError::Telegram(TelegramTransportError::Rpc(
                                "temporary stream disconnect".to_string(),
                            )))
                        } else {
                            Ok(vec![4, 5, 6])
                        }
                    }
                }
            },
        )
        .await
        .expect("stream read should recover within its window");

        assert_eq!(result.bytes, vec![4, 5, 6]);
        assert_eq!(result.retries, 5);
        assert_eq!(attempts.load(Ordering::SeqCst), 6);
    }

    #[tokio::test]
    async fn permanent_telegram_read_failure_is_not_retried() {
        let attempts = std::sync::Arc::new(AtomicUsize::new(0));
        let error =
            retry_telegram_read(RetryPolicy::new(3, std::time::Duration::ZERO, true), 18, {
                let attempts = std::sync::Arc::clone(&attempts);
                move || {
                    attempts.fetch_add(1, Ordering::SeqCst);
                    async {
                        Err(ObjectFormatError::InvalidRead(
                            "telegram message not found".to_string(),
                        ))
                    }
                }
            })
            .await
            .expect_err("missing message should remain a hard read failure");

        assert!(matches!(error, ObjectFormatError::InvalidRead(_)));
        assert_eq!(attempts.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn recovery_verifier_samples_distinct_random_chunk_indexes() {
        let sample = random_chunk_sample(100, 12);
        let mut sorted = sample.clone();
        sorted.sort_unstable();
        sorted.dedup();
        assert_eq!(sample.len(), 12);
        assert_eq!(sorted.len(), sample.len());
        assert!(sample.iter().all(|index| *index < 100));

        let all = random_chunk_sample(3, 99);
        assert_eq!(all.len(), 3);
        assert_eq!(
            all.iter()
                .copied()
                .collect::<std::collections::HashSet<_>>()
                .len(),
            3
        );
    }

    #[tokio::test]
    async fn changing_chunk_size_preserves_existing_object_reads() {
        let tempdir = TempDir::new().expect("tempdir");
        let service = sample_service(&tempdir).await;
        assert_eq!(
            service
                .metadata_store()
                .telegram_chunk_size()
                .expect("setting"),
            Some(AppConfig::default().chunk_size().expect("default chunk"))
        );

        service.set_chunk_size(1024).expect("small chunks");
        let payload = vec![b'x'; 2500];
        let manifest = service
            .put_bytes(
                "bucket",
                "chunk-policy.bin",
                "application/octet-stream",
                &payload,
            )
            .await
            .expect("put");
        assert_eq!(manifest.chunks.len(), 3);

        service.set_chunk_size(2048).expect("new chunks");
        assert_eq!(service.chunk_size(), 2048);
        assert_eq!(
            service
                .read_bytes("bucket", "chunk-policy.bin", 0..payload.len() as u64)
                .await
                .expect("read after policy change"),
            payload
        );
        assert_eq!(
            service.plan_upload(2500).expect("new plan").chunk_size,
            2048
        );
    }

    #[tokio::test]
    async fn first_chunk_priority_preserves_order_and_setting() {
        let tempdir = TempDir::new().expect("tempdir");
        let service = Arc::new(sample_service(&tempdir).await);
        service.set_chunk_size(1024).expect("small chunks");
        service
            .set_download_prefetch_chunks(2)
            .expect("prefetch setting");
        assert_eq!(service.download_prefetch_chunks(), 2);

        let payload = (0..4097)
            .map(|index| (index % 251) as u8)
            .collect::<Vec<_>>();
        let manifest = service
            .put_bytes(
                "bucket",
                "prefetch.bin",
                "application/octet-stream",
                &payload,
            )
            .await
            .expect("put");
        let plan =
            ObjectFormatService::plan_read(&manifest, 0..payload.len() as u64).expect("read plan");
        // The stream must still preserve object order after the first span is
        // delivered before the configured speculative window is opened.
        let pieces = ObjectFormatService::read_spans_to_stream(
            Arc::clone(&service),
            &manifest,
            plan.chunks,
            "test",
        )
        .collect::<Vec<_>>()
        .await;
        let actual = pieces
            .into_iter()
            .map(|piece| piece.expect("stream piece"))
            .flat_map(|piece| piece.to_vec())
            .collect::<Vec<_>>();

        assert_eq!(actual, payload);
        let metrics = service.download_stage_metrics();
        assert_eq!(metrics.completed_requests, 1);
        assert_eq!(metrics.failed_requests, 0);
        assert_eq!(metrics.recent[0].surface, "test");
        assert_eq!(metrics.recent[0].chunks, 5);
        assert_eq!(metrics.recent[0].client_bytes, payload.len() as u64);
        assert!(metrics.recent[0].first_chunk_us.is_some());
    }

    #[test]
    fn checksum_helpers_round_trip_hex() {
        let checksum = sha256_hex(b"hello world");
        let decoded = parse_checksum_hex(&checksum).expect("checksum");
        assert_eq!(decoded.len(), 32);
    }

    #[test]
    fn chunk_plan_covers_empty_and_all_boundary_lengths() {
        for (content_length, expected_sizes) in [
            (0, Vec::new()),
            (1, vec![1]),
            (1023, vec![1023]),
            (1024, vec![1024]),
            (1025, vec![1024, 1]),
            (2048, vec![1024, 1024]),
            (2049, vec![1024, 1024, 1]),
        ] {
            let plan = ObjectFormatService::plan_chunks(content_length, 1024).expect("plan");
            assert_eq!(
                plan.chunks
                    .iter()
                    .map(|chunk| chunk.size)
                    .collect::<Vec<_>>(),
                expected_sizes,
                "content length {content_length}"
            );
            assert_eq!(
                plan.chunks
                    .iter()
                    .map(|chunk| chunk.offset)
                    .collect::<Vec<_>>(),
                plan.chunks
                    .iter()
                    .scan(0, |offset, chunk| {
                        let current = *offset;
                        *offset += chunk.size;
                        Some(current)
                    })
                    .collect::<Vec<_>>(),
                "content length {content_length}"
            );
            assert_eq!(
                plan.chunks.iter().map(|chunk| chunk.size).sum::<u64>(),
                content_length
            );
        }
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
        let first_payload = b"hello ";
        let second_payload = b"multipart world";
        let first = service
            .upload_multipart_part(
                upload.upload_id,
                1,
                Some(s3s::dto::StreamingBlob::from_bytes(Bytes::from_static(
                    first_payload,
                ))),
                None,
            )
            .await
            .expect("upload first part");
        let second = service
            .upload_multipart_part(
                upload.upload_id,
                2,
                Some(s3s::dto::StreamingBlob::from_bytes(Bytes::from_static(
                    second_payload,
                ))),
                None,
            )
            .await
            .expect("upload second part");
        let abandoned_job = service
            .metadata
            .begin_transfer(Uuid::new_v4(), &upload.bucket, &upload.key)
            .expect("create abandoned pre-composition completion");
        service
            .metadata
            .with_connection(|connection| {
                connection.execute(
                    "INSERT INTO multipart_jobs(job_id,upload_id,part_number,expected_checksum) VALUES (?1,?2,0,NULL)",
                    rusqlite::params![abandoned_job, upload.upload_id.to_string()],
                )?;
                Ok(())
            })
            .expect("map abandoned completion job");
        assert_eq!(
            service
                .metadata
                .multipart_activity_state(upload.upload_id)
                .expect("derive finalizing state")
                .as_deref(),
            Some("completing")
        );
        let remote_dir = tempdir.path().join("data/mock-telegram");
        let remote_files_before = fs::read_dir(&remote_dir)
            .expect("mock Telegram directory")
            .filter_map(Result::ok)
            .filter(|entry| {
                entry.path().extension().and_then(|value| value.to_str()) == Some("bin")
            })
            .count();
        let plan = MultipartCompletionPlan {
            upload_id: upload.upload_id,
            object_id: upload.upload_id,
            bucket: upload.bucket.clone(),
            key: upload.key.clone(),
            content_type: upload.content_type.clone(),
            checksum_algorithm: upload.checksum_algorithm.clone(),
            content_length: first.size + second.size,
            parts: vec![
                MultipartPartPlan {
                    part_number: 1,
                    offset: 0,
                    size: first.size,
                    checksum: first.checksum.clone(),
                    e_tag: first.e_tag.clone(),
                },
                MultipartPartPlan {
                    part_number: 2,
                    offset: first.size,
                    size: second.size,
                    checksum: second.checksum.clone(),
                    e_tag: second.e_tag.clone(),
                },
            ],
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
        let abandoned = service
            .metadata
            .transfer(&abandoned_job)
            .expect("inspect abandoned completion")
            .expect("abandoned completion job");
        assert!(matches!(abandoned.state.as_str(), "superseded" | "cleaned"));
        assert_eq!(
            abandoned.error.as_deref(),
            Some("Superseded by metadata-only multipart completion")
        );
        assert_eq!(completed.schema_version, MANIFEST_SCHEMA_VERSION);
        assert_eq!(completed.checksum.algorithm, MULTIPART_CHECKSUM_ALGORITHM);
        assert_eq!(
            completed.checksum.whole_object,
            repeat.checksum.whole_object
        );
        assert!(
            completed
                .chunks
                .iter()
                .all(ChunkRef::references_remote_payload)
        );
        let source_chunks = first
            .manifest
            .as_ref()
            .expect("first manifest")
            .chunks
            .iter()
            .chain(
                second
                    .manifest
                    .as_ref()
                    .expect("second manifest")
                    .chunks
                    .iter(),
            )
            .collect::<Vec<_>>();
        assert_eq!(completed.chunks.len(), source_chunks.len());
        for (composed, source) in completed.chunks.iter().zip(source_chunks) {
            assert_eq!(
                composed.payload_identity(completed.object_id),
                source.payload_identity(if composed.offset < first.size {
                    first.manifest.as_ref().expect("first manifest").object_id
                } else {
                    second.manifest.as_ref().expect("second manifest").object_id
                })
            );
            assert_eq!(composed.telegram_message_id, source.telegram_message_id);
        }
        let remote_files_after = fs::read_dir(&remote_dir)
            .expect("mock Telegram directory")
            .filter_map(Result::ok)
            .filter(|entry| {
                entry.path().extension().and_then(|value| value.to_str()) == Some("bin")
            })
            .count();
        assert_eq!(
            remote_files_after,
            remote_files_before + 1,
            "completion should upload only the final manifest"
        );
        let expected = [first_payload.as_slice(), second_payload.as_slice()].concat();
        assert_eq!(
            service
                .read_bytes("bucket", "multipart.txt", 0..expected.len() as u64)
                .await
                .expect("read composed multipart object"),
            expected
        );
        assert_eq!(
            service
                .read_bytes("bucket", "multipart.txt", 4..12)
                .await
                .expect("read across part boundary"),
            b"o multip"
        );

        for retained in &completed.chunks {
            let cleanup_count: i64 = service
                .metadata
                .with_connection(|connection| {
                    Ok(connection.query_row(
                        "SELECT COUNT(*) FROM cleanup_targets WHERE peer_id=?1 AND message_id=?2",
                        rusqlite::params![retained.telegram_peer_id, retained.telegram_message_id],
                        |row| row.get(0),
                    )?)
                })
                .expect("inspect retained cleanup targets");
            assert_eq!(cleanup_count, 0, "retained chunks must not be deleted");
        }
        for part_manifest in [&first.telegram, &second.telegram] {
            let cleanup_count: i64 = service
                .metadata
                .with_connection(|connection| {
                    Ok(connection.query_row(
                        "SELECT COUNT(*) FROM cleanup_targets WHERE peer_id=?1 AND message_id=?2",
                        rusqlite::params![part_manifest.peer_id, part_manifest.message_id],
                        |row| row.get(0),
                    )?)
                })
                .expect("inspect part manifest cleanup target");
            assert_eq!(cleanup_count, 1, "part manifests should be reclaimed");
        }

        service.shutdown_workers().await;
        drop(service);
        let service = sample_service(&tempdir).await;
        assert_eq!(
            service
                .read_bytes("bucket", "multipart.txt", 0..expected.len() as u64)
                .await
                .expect("read composed object after restart"),
            expected
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
        assert_eq!(status.telegram_files_bytes, 3);
    }

    #[tokio::test]
    async fn traffic_metrics_separate_session_and_persisted_total() {
        let tempdir = TempDir::new().expect("tempdir");
        unsafe {
            env::set_var("TELEGRAM_TRANSPORT_RUNTIME", "mock");
        }
        let config = test_config(&tempdir);
        let service = ObjectFormatService::open(&config).await.expect("service");
        service.add_client_upload_bytes(10);
        assert_eq!(service.traffic_metrics().session.client_upload_bytes, 10);
        assert_eq!(service.traffic_metrics().total.client_upload_bytes, 10);
        drop(service);

        let reopened = ObjectFormatService::open(&config).await.expect("reopen");
        let metrics = reopened.traffic_metrics();
        assert_eq!(metrics.session.client_upload_bytes, 0);
        assert_eq!(metrics.total.client_upload_bytes, 10);
    }

    #[test]
    fn telegram_files_bytes_counts_unique_committed_chunk_locations() {
        let reused = ObjectManifest::committed(CommittedManifestArgs {
            bucket: "bucket".to_string(),
            key: "one.txt".to_string(),
            content_length: 3,
            content_type: "text/plain".to_string(),
            checksum_algorithm: CHECKSUM_ALGORITHM.to_string(),
            whole_object: "one".to_string(),
            peer_id: "-1001234567890".to_string(),
            message_id: 1,
        });
        let duplicate_reference = ObjectManifest::committed(CommittedManifestArgs {
            bucket: "bucket".to_string(),
            key: "two.txt".to_string(),
            content_length: 3,
            content_type: "text/plain".to_string(),
            checksum_algorithm: CHECKSUM_ALGORITHM.to_string(),
            whole_object: "two".to_string(),
            peer_id: "-1001234567890".to_string(),
            message_id: 1,
        });
        let mut not_committed = ObjectManifest::committed(CommittedManifestArgs {
            bucket: "bucket".to_string(),
            key: "staged.txt".to_string(),
            content_length: 9,
            content_type: "text/plain".to_string(),
            checksum_algorithm: CHECKSUM_ALGORITHM.to_string(),
            whole_object: "staged".to_string(),
            peer_id: "-1001234567890".to_string(),
            message_id: 2,
        });
        not_committed.commit_state = CommitState::Staging;
        let distinct = ObjectManifest::committed(CommittedManifestArgs {
            bucket: "bucket".to_string(),
            key: "three.txt".to_string(),
            content_length: 4,
            content_type: "text/plain".to_string(),
            checksum_algorithm: CHECKSUM_ALGORITHM.to_string(),
            whole_object: "three".to_string(),
            peer_id: "-1001234567890".to_string(),
            message_id: 3,
        });

        assert_eq!(
            telegram_files_bytes(&[reused, duplicate_reference, not_committed, distinct]),
            7
        );
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
    async fn verifier_quarantines_an_object_after_confirmed_sample_failure() {
        let tempdir = TempDir::new().expect("tempdir");
        let service = sample_service(&tempdir).await;
        service
            .set_recovery_verification_settings(60, 3)
            .expect("verifier settings");
        let manifest = service
            .put_bytes(
                "bucket",
                "sampled.txt",
                "text/plain",
                &vec![7_u8; 2_100_000],
            )
            .await
            .expect("put object");
        let failed_message = manifest.chunks[1].telegram_message_id;
        fs::write(
            tempdir
                .path()
                .join(format!("data/mock-telegram/{failed_message}.bin")),
            b"corrupt sampled payload",
        )
        .expect("corrupt remote chunk");

        service
            .refresh_recovery_snapshot()
            .await
            .expect("refresh verifier");

        assert!(
            service
                .get_active_manifest("bucket", "sampled.txt")
                .expect("active manifest")
                .is_none()
        );
        assert_eq!(
            service
                .metadata
                .get_manifest(manifest.object_id)
                .expect("manifest")
                .expect("stored manifest")
                .commit_state,
            CommitState::RecoveryRequired
        );
        let snapshot = service.cached_recovery_snapshot().expect("snapshot");
        assert!(
            snapshot
                .1
                .iter()
                .any(|issue| issue.kind == "corrupted_chunk")
        );
    }

    #[tokio::test]
    async fn opening_service_defers_remote_recovery_scan() {
        let tempdir = TempDir::new().expect("tempdir");
        let service = sample_service(&tempdir).await;
        service
            .put_bytes("bucket", "startup.txt", "text/plain", b"startup")
            .await
            .expect("put object");
        service.shutdown_workers().await;
        drop(service);

        let reopened = sample_service(&tempdir).await;
        let snapshot = reopened.cached_recovery_snapshot().expect("snapshot");
        assert!(snapshot.0.is_none());
        assert_eq!(snapshot.1.len(), 0);
        assert_eq!(snapshot.2.as_deref(), Some("Recovery scan pending"));
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
    async fn mock_remote_reconciliation_requires_token_and_exact_bytes() {
        let tempdir = TempDir::new().expect("tempdir");
        let service = sample_service(&tempdir).await;
        let local = tempdir.path().join("data/reconcile.bin");
        fs::write(&local, b"local encrypted bytes").expect("local bytes");
        let remote = tempdir.path().join("data/mock-telegram");
        fs::create_dir_all(&remote).expect("mock directory");
        fs::write(remote.join("41.bin"), b"local encrypted bytes").expect("remote bytes");
        fs::write(
            remote.join("41.json"),
            serde_json::to_vec(&serde_json::json!({
                "file_name": "telegram-s3-test-token"
            }))
            .expect("sidecar"),
        )
        .expect("remote sidecar");

        let match_result = service
            .reconcile_remote_file(&local, "telegram-s3-test-token", 0)
            .await
            .expect("matching remote document");
        assert!(matches!(
            match_result,
            RemoteReconciliation::Match(TelegramLocation { message_id: 41, .. })
        ));

        fs::write(remote.join("42.bin"), b"different bytes").expect("collision bytes");
        fs::write(
            remote.join("42.json"),
            serde_json::to_vec(&serde_json::json!({
                "file_name": "telegram-s3-collision-token"
            }))
            .expect("collision sidecar"),
        )
        .expect("collision remote sidecar");
        let collision = service
            .reconcile_remote_file(&local, "telegram-s3-collision-token", 0)
            .await
            .expect_err("byte collision must remain recovery-required");
        assert!(collision.to_string().contains("token collision"));

        let absent = service
            .reconcile_remote_file(&local, "telegram-s3-absent-token", 0)
            .await
            .expect("absent scan");
        assert!(matches!(absent, RemoteReconciliation::Absent));
    }

    #[tokio::test]
    async fn corrupt_mock_remote_payload_is_rejected_on_read() {
        let tempdir = TempDir::new().expect("tempdir");
        let service = sample_service(&tempdir).await;
        let manifest = service
            .put_bytes("bucket", "corrupt.txt", "text/plain", b"verified payload")
            .await
            .expect("put");
        let chunk = manifest.chunks.first().expect("chunk").telegram_message_id;
        fs::write(
            tempdir
                .path()
                .join(format!("data/mock-telegram/{chunk}.bin")),
            b"corrupted remote bytes",
        )
        .expect("corrupt remote payload");

        let error = service
            .read_bytes("bucket", "corrupt.txt", 0..manifest.content_length)
            .await
            .expect_err("corrupt remote payload must fail closed");
        assert!(
            error.to_string().contains("decrypt")
                || error.to_string().contains("checksum")
                || error.to_string().contains("aead")
        );
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
