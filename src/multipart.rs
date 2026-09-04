use crate::manifest::TelegramLocation;
use serde::{Deserialize, Serialize};
use time::OffsetDateTime;
use uuid::Uuid;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MultipartState {
    Initiated,
    Uploading,
    Completing,
    Completed,
    Aborted,
    RecoveryRequired,
    Quarantined,
}

impl MultipartState {
    pub fn as_str(self) -> &'static str {
        match self {
            MultipartState::Initiated => "initiated",
            MultipartState::Uploading => "uploading",
            MultipartState::Completing => "completing",
            MultipartState::Completed => "completed",
            MultipartState::Aborted => "aborted",
            MultipartState::RecoveryRequired => "recovery_required",
            MultipartState::Quarantined => "quarantined",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MultipartSession {
    pub upload_id: Uuid,
    pub bucket: String,
    pub key: String,
    pub version_id: Option<String>,
    pub content_type: String,
    pub checksum_algorithm: String,
    pub state: MultipartState,
    pub created_at: OffsetDateTime,
    pub updated_at: OffsetDateTime,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MultipartPart {
    pub upload_id: Uuid,
    pub part_number: u32,
    pub size: u64,
    pub checksum: String,
    pub e_tag: String,
    pub telegram: TelegramLocation,
    pub created_at: OffsetDateTime,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MultipartPartPlan {
    pub part_number: u32,
    pub offset: u64,
    pub size: u64,
    pub checksum: String,
    pub e_tag: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MultipartCompletionPlan {
    pub upload_id: Uuid,
    pub object_id: Uuid,
    pub bucket: String,
    pub key: String,
    pub content_type: String,
    pub checksum_algorithm: String,
    pub content_length: u64,
    pub parts: Vec<MultipartPartPlan>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct MultipartReconciliationReport {
    pub sessions_scanned: u64,
    pub sessions_recovered: u64,
    pub sessions_quarantined: u64,
    pub orphaned_parts: u64,
}
