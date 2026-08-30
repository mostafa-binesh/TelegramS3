use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use time::OffsetDateTime;
use uuid::Uuid;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CommitState {
    Staging,
    Committed,
    Tombstoned,
    Orphaned,
    RecoveryRequired,
}

impl CommitState {
    pub fn as_str(self) -> &'static str {
        match self {
            CommitState::Staging => "staging",
            CommitState::Committed => "committed",
            CommitState::Tombstoned => "tombstoned",
            CommitState::Orphaned => "orphaned",
            CommitState::RecoveryRequired => "recovery_required",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ObjectChecksum {
    pub algorithm: String,
    pub whole_object: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EncryptionInfo {
    pub enabled: bool,
    pub format: String,
    pub key_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChunkRef {
    pub order: u32,
    pub offset: u64,
    pub size: u64,
    pub checksum: String,
    pub telegram_peer_id: String,
    pub telegram_message_id: i64,
    pub telegram_document_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TelegramLocation {
    pub peer_id: String,
    pub message_id: i64,
    pub document_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ObjectManifest {
    pub schema_version: u16,
    pub commit_state: CommitState,
    pub object_id: Uuid,
    pub bucket: String,
    pub key: String,
    pub version_id: Option<String>,
    pub content_length: u64,
    pub content_type: String,
    pub user_metadata: BTreeMap<String, String>,
    pub tags: BTreeMap<String, String>,
    pub created_at: OffsetDateTime,
    pub checksum: ObjectChecksum,
    pub encryption: EncryptionInfo,
    pub telegram: TelegramLocation,
    pub chunks: Vec<ChunkRef>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommittedManifestArgs {
    pub bucket: String,
    pub key: String,
    pub content_length: u64,
    pub content_type: String,
    pub checksum_algorithm: String,
    pub whole_object: String,
    pub peer_id: String,
    pub message_id: i64,
}

impl ObjectManifest {
    pub fn committed(args: CommittedManifestArgs) -> Self {
        let chunks = if args.content_length == 0 {
            Vec::new()
        } else {
            vec![ChunkRef {
                order: 0,
                offset: 0,
                size: args.content_length,
                checksum: args.whole_object.clone(),
                telegram_peer_id: args.peer_id.clone(),
                telegram_message_id: args.message_id + 1,
                telegram_document_id: None,
            }]
        };
        Self {
            schema_version: 1,
            commit_state: CommitState::Committed,
            object_id: Uuid::new_v4(),
            bucket: args.bucket,
            key: args.key,
            version_id: Some(Uuid::new_v4().to_string()),
            content_length: args.content_length,
            content_type: args.content_type,
            user_metadata: BTreeMap::new(),
            tags: BTreeMap::new(),
            created_at: OffsetDateTime::now_utc(),
            checksum: ObjectChecksum {
                algorithm: args.checksum_algorithm,
                whole_object: args.whole_object,
            },
            encryption: EncryptionInfo {
                enabled: false,
                format: "none".to_string(),
                key_id: None,
            },
            telegram: TelegramLocation {
                peer_id: args.peer_id,
                message_id: args.message_id,
                document_id: None,
            },
            chunks,
        }
    }

    pub fn validate(&self) -> Result<(), String> {
        if self.schema_version == 0 {
            return Err("schema_version must be non-zero".to_string());
        }
        if self.bucket.trim().is_empty() {
            return Err("bucket is required".to_string());
        }
        if self.key.trim().is_empty() {
            return Err("key is required".to_string());
        }
        if self.checksum.algorithm.trim().is_empty() {
            return Err("checksum.algorithm is required".to_string());
        }
        if self.checksum.whole_object.trim().is_empty() {
            return Err("checksum.whole_object is required".to_string());
        }
        if self.telegram.peer_id.trim().is_empty() {
            return Err("telegram.peer_id is required".to_string());
        }
        if self.chunks.is_empty() {
            if self.content_length != 0 {
                return Err("non-empty content_length requires chunk references".to_string());
            }
            return Ok(());
        }

        let mut expected_offset = 0_u64;
        for (index, chunk) in self.chunks.iter().enumerate() {
            if chunk.order != index as u32 {
                return Err("chunk order must be contiguous".to_string());
            }
            if chunk.size == 0 {
                return Err("chunk size must be non-zero".to_string());
            }
            if chunk.offset != expected_offset {
                return Err("chunk offsets must be contiguous".to_string());
            }
            if chunk.checksum.trim().is_empty() {
                return Err("chunk checksum is required".to_string());
            }
            if chunk.telegram_peer_id.trim().is_empty() {
                return Err("chunk telegram_peer_id is required".to_string());
            }
            expected_offset = expected_offset
                .checked_add(chunk.size)
                .ok_or_else(|| "chunk sizes overflow content length".to_string())?;
        }
        if expected_offset != self.content_length {
            return Err("chunk sizes must sum to content_length".to_string());
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip_manifest_json() {
        let manifest = ObjectManifest::committed(CommittedManifestArgs {
            bucket: "bucket".to_string(),
            key: "key.txt".to_string(),
            content_length: 12,
            content_type: "text/plain".to_string(),
            checksum_algorithm: "sha256".to_string(),
            whole_object: "deadbeef".to_string(),
            peer_id: "peer".to_string(),
            message_id: 42,
        });
        let json = serde_json::to_string(&manifest).expect("serialize");
        let restored: ObjectManifest = serde_json::from_str(&json).expect("deserialize");

        assert_eq!(restored.bucket, "bucket");
        assert_eq!(restored.chunks.len(), 1);
        assert!(restored.validate().is_ok());
    }
}
