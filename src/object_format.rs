use crate::config::AppConfig;
use crate::manifest::{ChunkRef, CommitState, ObjectChecksum, ObjectManifest};
use crate::metadata::{JournalEntry, MetadataError, MetadataStatus, MetadataStore, OperationKind};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, HashMap};
use std::fs::{self, File};
use std::io::{self, Read, Write};
use std::ops::Range;
use std::path::{Path, PathBuf};
use thiserror::Error;
use time::OffsetDateTime;
use uuid::Uuid;

const CHECKSUM_ALGORITHM: &str = "sha256";
const MANIFEST_FILE_NAME: &str = "manifest.json";
const STAGING_ROOT: &str = "staging";
const MANIFEST_ROOT: &str = "manifests";
const CHUNK_ROOT: &str = "chunks";
const QUARANTINE_ROOT: &str = "quarantine";

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

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReconciliationReport {
    pub staged_objects: u64,
    pub committed_objects: u64,
    pub recovery_required_objects: u64,
    pub orphaned_chunks: u64,
    pub repaired_objects: u64,
    pub quarantined_objects: u64,
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

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StagedObject {
    pub operation_id: Uuid,
    pub object_id: Uuid,
    pub manifest: ObjectManifest,
    pub chunk_plan: ChunkPlan,
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
}

pub struct ObjectFormatService {
    metadata: MetadataStore,
    data_dir: PathBuf,
    chunk_size: u64,
    storage_chat_id: String,
}

impl ObjectFormatService {
    pub fn open(config: &AppConfig) -> Result<Self, ObjectFormatError> {
        config.validate()?;
        let metadata = MetadataStore::open(config.metadata_path())?;
        let service = Self::new(
            metadata,
            config.data_dir(),
            config.chunk_size()?,
            config.telegram_storage_chat_id.clone().ok_or_else(|| {
                ObjectFormatError::InvalidPlan("missing storage chat id".to_string())
            })?,
        )?;
        Ok(service)
    }

    pub fn new(
        metadata: MetadataStore,
        data_dir: impl AsRef<Path>,
        chunk_size: u64,
        storage_chat_id: String,
    ) -> Result<Self, ObjectFormatError> {
        let data_dir = data_dir.as_ref().to_path_buf();
        fs::create_dir_all(data_dir.join(STAGING_ROOT))?;
        fs::create_dir_all(data_dir.join(MANIFEST_ROOT))?;
        fs::create_dir_all(data_dir.join(CHUNK_ROOT))?;
        fs::create_dir_all(data_dir.join(QUARANTINE_ROOT))?;
        Ok(Self {
            metadata,
            data_dir,
            chunk_size,
            storage_chat_id,
        })
    }

    pub fn metadata_status(&self) -> Result<MetadataStatus, ObjectFormatError> {
        Ok(self.metadata.status()?)
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

    pub fn bootstrap(&self) -> Result<ObjectFormatStatus, ObjectFormatError> {
        let report = self.reconcile()?;
        if report.staged_objects > 0 || report.recovery_required_objects > 0 {
            return Err(ObjectFormatError::RecoveryRequired(format!(
                "staged_objects={}, recovery_required_objects={}",
                report.staged_objects, report.recovery_required_objects
            )));
        }
        self.status()
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
        let object_id = Uuid::new_v4();
        let scratch_dir = self
            .data_dir
            .join(STAGING_ROOT)
            .join(format!("upload-{object_id}"));
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
            let mut chunk_file = File::create(&chunk_path)?;
            chunk_file.write_all(chunk_bytes)?;
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
                telegram_peer_id: self.storage_chat_id.clone(),
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

        let whole_checksum = sha256_hex(whole_hasher.finalize());
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
        }
        let stage_manifest_path = staging_dir.join(MANIFEST_FILE_NAME);
        write_json_file(&stage_manifest_path, &manifest)?;
        verify_staged_chunks(&staging_dir, &manifest)?;
        Ok(StagedObject {
            operation_id,
            object_id,
            manifest,
            chunk_plan,
        })
    }

    pub fn commit_staged_object(
        &self,
        staged: &StagedObject,
    ) -> Result<ObjectManifest, ObjectFormatError> {
        let staging_dir = self.staging_dir(staged.operation_id);
        let committed_chunk_dir = self.chunk_dir(staged.object_id);
        let committed_manifest_path = self.manifest_path(staged.object_id);
        fs::create_dir_all(&committed_chunk_dir)?;

        for chunk in &staged.manifest.chunks {
            let staged_path = staging_dir.join(chunk_file_name(chunk.order));
            let committed_path = committed_chunk_dir.join(chunk_file_name(chunk.order));
            if !staged_path.exists() {
                return Err(ObjectFormatError::MissingChunk {
                    object_id: staged.object_id,
                    order: chunk.order,
                });
            }
            fs::rename(&staged_path, &committed_path)?;
        }

        let mut committed_manifest = staged.manifest.clone();
        committed_manifest.commit_state = CommitState::Committed;
        write_json_file(&committed_manifest_path, &committed_manifest)?;
        let committed = self.metadata.commit_manifest(staged.operation_id)?;
        match fs::remove_dir_all(&staging_dir) {
            Ok(()) => {}
            Err(error) if error.kind() != io::ErrorKind::NotFound => {
                return Err(error.into());
            }
            Err(_) => {}
        }
        Ok(committed)
    }

    pub fn put_bytes(
        &self,
        bucket: &str,
        key: &str,
        content_type: &str,
        bytes: &[u8],
    ) -> Result<ObjectManifest, ObjectFormatError> {
        let staged = self.stage_bytes(bucket, key, content_type, bytes)?;
        self.commit_staged_object(&staged)
    }

    pub fn read_bytes(
        &self,
        bucket: &str,
        key: &str,
        range: Range<u64>,
    ) -> Result<Vec<u8>, ObjectFormatError> {
        let mut output = Vec::new();
        self.read_range_to_writer(bucket, key, range, &mut output)?;
        Ok(output)
    }

    pub fn read_range_to_writer<W: Write>(
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
        let plan = Self::plan_read(&manifest, range.clone())?;
        for span in plan.chunks {
            let chunk = manifest.chunks.get(span.order as usize).ok_or(
                ObjectFormatError::MissingChunk {
                    object_id: manifest.object_id,
                    order: span.order,
                },
            )?;
            let path = self
                .chunk_dir(manifest.object_id)
                .join(chunk_file_name(chunk.order));
            if !path.exists() {
                return Err(ObjectFormatError::MissingChunk {
                    object_id: manifest.object_id,
                    order: chunk.order,
                });
            }
            let mut file = File::open(&path)?;
            let mut chunk_bytes = Vec::new();
            file.read_to_end(&mut chunk_bytes)?;
            let actual_checksum = sha256_hex(&chunk_bytes);
            if actual_checksum != chunk.checksum {
                return Err(ObjectFormatError::ChecksumMismatch {
                    scope: format!("chunk {}", chunk.order),
                    expected: chunk.checksum.clone(),
                    actual: actual_checksum,
                });
            }
            let start = span.offset_within_chunk as usize;
            let end = start + span.length as usize;
            writer.write_all(&chunk_bytes[start..end])?;
        }
        Ok(())
    }

    pub fn reconcile(&self) -> Result<ReconciliationReport, ObjectFormatError> {
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
                        self.commit_staged_object(&staged)?;
                        repaired_objects += 1;
                        continue;
                    }
                    self.metadata
                        .update_manifest_state(manifest.object_id, CommitState::RecoveryRequired)?;
                    quarantined_objects += self.quarantine_staging_manifest(manifest)?;
                }
                CommitState::Committed => {
                    if !self.committed_object_ready(manifest)? {
                        self.metadata.update_manifest_state(
                            manifest.object_id,
                            CommitState::RecoveryRequired,
                        )?;
                        repaired_objects += self.try_repair_committed_manifest(manifest)?;
                    }
                }
                CommitState::RecoveryRequired => {
                    if self.committed_object_ready(manifest)? {
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

    fn committed_object_ready(&self, manifest: &ObjectManifest) -> Result<bool, ObjectFormatError> {
        let manifest_path = self.manifest_path(manifest.object_id);
        if !manifest_path.exists() {
            return Ok(false);
        }
        for chunk in &manifest.chunks {
            let path = self
                .chunk_dir(manifest.object_id)
                .join(chunk_file_name(chunk.order));
            if !path.exists() {
                return Ok(false);
            }
            let mut file = File::open(&path)?;
            let mut bytes = Vec::new();
            file.read_to_end(&mut bytes)?;
            if sha256_hex(&bytes) != chunk.checksum {
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
            if sha256_hex(&bytes) != chunk.checksum {
                return Ok(false);
            }
        }
        Ok(true)
    }

    fn try_repair_committed_manifest(
        &self,
        manifest: &ObjectManifest,
    ) -> Result<u64, ObjectFormatError> {
        if self.committed_object_ready(manifest)? {
            let committed_manifest_path = self.manifest_path(manifest.object_id);
            write_json_file(&committed_manifest_path, manifest)?;
            self.metadata
                .update_manifest_state(manifest.object_id, CommitState::Committed)?;
            return Ok(1);
        }
        Ok(0)
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
            version_id: None,
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
                enabled: false,
                format: "none".to_string(),
                key_id: None,
            },
            telegram: crate::manifest::TelegramLocation {
                peer_id: self.storage_chat_id.clone(),
                message_id: 0,
                document_id: Some(format!("local:{}:manifest", args.object_id)),
            },
            chunks: args.chunks,
        }
    }
}

fn verify_staged_chunks(
    staging_dir: &Path,
    manifest: &ObjectManifest,
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
        let actual_checksum = sha256_hex(&bytes);
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
    let mut file = File::create(path)?;
    let bytes = serde_json::to_vec_pretty(value)?;
    file.write_all(&bytes)?;
    file.sync_all()?;
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
    use tempfile::TempDir;

    fn sample_service(tempdir: &TempDir) -> ObjectFormatService {
        let metadata = MetadataStore::open_in_memory().expect("metadata");
        ObjectFormatService::new(metadata, tempdir.path(), 1024, "-1001234567890".to_string())
            .expect("service")
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

    #[test]
    fn put_and_read_range_are_chunk_aware() {
        let tempdir = TempDir::new().expect("tempdir");
        let service = sample_service(&tempdir);
        let payload = b"abcdefghijklmnopqrstuvwxyz";
        let manifest = service
            .put_bytes("bucket", "key.txt", "text/plain", payload)
            .expect("put");
        assert_eq!(manifest.commit_state, CommitState::Committed);
        let bytes = service
            .read_bytes("bucket", "key.txt", 3..19)
            .expect("read");
        assert_eq!(&bytes, b"defghijklmnopqrs");
    }

    #[test]
    fn staged_upload_can_be_reconciled_after_restart() {
        let tempdir = TempDir::new().expect("tempdir");
        let metadata =
            MetadataStore::open(tempdir.path().join("metadata.sqlite")).expect("metadata");
        let service = ObjectFormatService::new(
            metadata,
            tempdir.path().join("data"),
            8,
            "-1001234567890".to_string(),
        )
        .expect("service");

        let staged = service
            .stage_bytes("bucket", "key.txt", "text/plain", b"hello world")
            .expect("stage");
        assert_eq!(staged.manifest.commit_state, CommitState::Staging);
        let report = service.reconcile().expect("reconcile");
        assert_eq!(report.repaired_objects, 1);
        assert_eq!(report.staged_objects, 0);
    }
}
