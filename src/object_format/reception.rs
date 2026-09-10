use super::*;
use crate::durable::TransferJob;

#[derive(Debug, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub struct ReceptionStatus {
    pub id: String,
    pub received: u64,
    pub chunk_size: u64,
}

pub(crate) struct ReceptionState {
    pub object_id: Uuid,
    pub bucket: String,
    pub key: String,
    pub content_type: String,
    pub expires_at: Option<OffsetDateTime>,
    pub chunks: Vec<ChunkRef>,
    pub hasher: Sha256,
    pub offset: u64,
    pub final_received: bool,
    pub chunk_size: u64,
}

impl ObjectFormatService {
    pub async fn begin_reception(
        &self,
        bucket: &str,
        key: &str,
        content_type: &str,
    ) -> Result<ReceptionStatus, ObjectFormatError> {
        self.begin_reception_with_expiry(bucket, key, content_type, None)
            .await
    }

    pub async fn begin_reception_with_expiry(
        &self,
        bucket: &str,
        key: &str,
        content_type: &str,
        expires_at: Option<OffsetDateTime>,
    ) -> Result<ReceptionStatus, ObjectFormatError> {
        self.ensure_connection_not_removing()?;
        let object_id = Uuid::new_v4();
        let chunk_size = self.chunk_size();
        let id = self.metadata.begin_transfer(object_id, bucket, key)?;
        let dir = self.staging_dir(object_id);
        if let Err(error) = async_fs::create_dir_all(&dir).await {
            let _ = self.metadata.fail_reception(
                &id,
                true,
                "resumable reception staging could not be created",
            );
            return Err(error.into());
        }
        let state = ReceptionState {
            object_id,
            bucket: bucket.to_string(),
            key: key.to_string(),
            content_type: content_type.to_string(),
            expires_at,
            chunks: Vec::new(),
            hasher: Sha256::new(),
            offset: 0,
            final_received: false,
            chunk_size,
        };
        self.receptions
            .lock()
            .expect("reception map lock")
            .insert(object_id, Arc::new(tokio::sync::Mutex::new(state)));
        Ok(ReceptionStatus {
            id,
            received: 0,
            chunk_size,
        })
    }

    pub async fn reception_status(&self, id: &str) -> Result<ReceptionStatus, ObjectFormatError> {
        let object_id = parse_reception_id(id)?;
        let active = self
            .metadata
            .transfer(id)?
            .is_some_and(|job| job.state == "receiving");
        if !active {
            return Err(ObjectFormatError::ReceptionNotFound);
        }
        let state = self.reception_state(object_id)?;
        let state = state.lock().await;
        Ok(ReceptionStatus {
            id: id.to_string(),
            received: state.offset,
            chunk_size: state.chunk_size,
        })
    }

    pub async fn receive_reception_chunk(
        &self,
        id: &str,
        offset: u64,
        bytes: Bytes,
        final_chunk: bool,
    ) -> Result<ReceptionStatus, ObjectFormatError> {
        let object_id = parse_reception_id(id)?;
        let state = self.reception_state(object_id)?;
        let mut state = state.lock().await;
        if state.offset != offset {
            return Err(ObjectFormatError::ReceptionOffsetMismatch {
                received: state.offset,
            });
        }
        if bytes.len() as u64 > state.chunk_size {
            return Err(ObjectFormatError::ReceptionChunkTooLarge);
        }
        if !final_chunk && bytes.len() as u64 != state.chunk_size {
            return Err(ObjectFormatError::ReceptionChunkSizeMismatch);
        }
        if !bytes.is_empty() {
            let object_id = state.object_id;
            let ReceptionState {
                chunks,
                hasher,
                offset,
                ..
            } = &mut *state;
            self.stage_transfer_chunk(object_id, &bytes, chunks, hasher, offset)
                .await?;
        }
        if final_chunk {
            state.final_received = true;
        }
        Ok(ReceptionStatus {
            id: id.to_string(),
            received: state.offset,
            chunk_size: state.chunk_size,
        })
    }

    pub async fn finish_reception(&self, id: &str) -> Result<TransferJob, ObjectFormatError> {
        let object_id = parse_reception_id(id)?;
        let state = self.reception_state(object_id)?;
        let mut state = state.lock().await;
        if !state.final_received {
            return Err(ObjectFormatError::ReceptionNotComplete);
        }
        let checksum = hex::encode(std::mem::replace(&mut state.hasher, Sha256::new()).finalize());
        let object_id = state.object_id;
        let bucket = state.bucket.clone();
        let key = state.key.clone();
        let content_type = state.content_type.clone();
        let expires_at = state.expires_at;
        let chunks = std::mem::take(&mut state.chunks);
        let result = self.finalize_reception(
            id,
            ManifestBuildArgs {
                object_id,
                bucket,
                key,
                content_type,
                expires_at,
                commit_state: CommitState::Staging,
                chunks,
                whole_checksum: checksum,
            },
            None,
        );
        drop(state);
        match result {
            Ok(job) => {
                self.remove_reception(object_id);
                self.ensure_workers();
                Ok(job)
            }
            Err(error) => {
                self.cleanup_failed_reception(id, object_id).await;
                Err(error)
            }
        }
    }

    pub async fn abort_reception(&self, id: &str) -> Result<(), ObjectFormatError> {
        let object_id = parse_reception_id(id)?;
        let state = self.reception_state(object_id)?;
        let state = state.lock().await;
        drop(state);
        self.remove_reception(object_id);
        let dir = self.staging_dir(object_id);
        let removed = match async_fs::remove_dir_all(dir).await {
            Ok(()) => true,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => true,
            Err(_) => false,
        };
        self.metadata
            .fail_reception(id, removed, "resumable reception cancelled")?;
        Ok(())
    }

    fn reception_state(
        &self,
        object_id: Uuid,
    ) -> Result<Arc<tokio::sync::Mutex<ReceptionState>>, ObjectFormatError> {
        self.receptions
            .lock()
            .expect("reception map lock")
            .get(&object_id)
            .cloned()
            .ok_or(ObjectFormatError::ReceptionNotFound)
    }

    fn remove_reception(&self, object_id: Uuid) {
        self.receptions
            .lock()
            .expect("reception map lock")
            .remove(&object_id);
    }

    async fn cleanup_failed_reception(&self, id: &str, object_id: Uuid) {
        let removed = match async_fs::remove_dir_all(self.staging_dir(object_id)).await {
            Ok(()) => true,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => true,
            Err(_) => false,
        };
        let _ = self.metadata.fail_reception(
            id,
            removed,
            "Upload reception failed before durable acceptance; resend the source file",
        );
        self.remove_reception(object_id);
    }
}

fn parse_reception_id(id: &str) -> Result<Uuid, ObjectFormatError> {
    Uuid::parse_str(id).map_err(|_| ObjectFormatError::ReceptionNotFound)
}
