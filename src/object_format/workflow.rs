use super::*;
use crate::durable::{CleanupTarget, TransferJob, TransferWriteConditionals};
use s3s::Body;
use std::sync::atomic::Ordering;

#[derive(Debug)]
pub(crate) enum RemoteReconciliation {
    Match(TelegramLocation),
    Absent,
}

// Telegram can reject `messages.sendMedia` with FLOOD_WAIT before it accepts a
// document. When the server does not include a wait value, pause
// conservatively instead of immediately putting pressure back on the account.
const FALLBACK_FLOOD_WAIT_SECONDS: u64 = 60;

fn is_explicit_flood_wait(error: &ObjectFormatError) -> bool {
    matches!(
        error,
        ObjectFormatError::Telegram(crate::telegram::TelegramTransportError::Rpc(detail))
            if crate::telegram::retry::is_flood_wait_error(detail)
    )
}

pub(crate) fn can_reuse_replica_message(
    source_peer_id: &str,
    target_storage_chat_id: Option<&str>,
) -> bool {
    target_storage_chat_id == Some(source_peer_id)
}

impl ObjectFormatService {
    /// Drain a duplicate S3 request without staging it. The already-durable
    /// multipart job remains the source of truth, so a broken retry body must
    /// not discard or poison that earlier job.
    pub(super) async fn discard_duplicate_body(&self, mut body: Option<StreamingBlob>) {
        let Some(body) = body.as_mut() else {
            return;
        };
        while let Some(frame) = body.next().await {
            match frame {
                Ok(bytes) => self.add_client_upload_bytes(bytes.len() as u64),
                Err(_) => break,
            }
        }
    }

    /// Accept the complete request on durable local storage; remote work is independent.
    pub async fn enqueue_stream(
        &self,
        bucket: &str,
        key: &str,
        content_type: &str,
        body: Option<StreamingBlob>,
        conditionals: Option<TransferWriteConditionals>,
    ) -> Result<TransferJob, ObjectFormatError> {
        self.enqueue_stream_with_expiry(bucket, key, content_type, body, conditionals, None)
            .await
    }

    pub async fn enqueue_stream_with_expiry(
        &self,
        bucket: &str,
        key: &str,
        content_type: &str,
        body: Option<StreamingBlob>,
        conditionals: Option<TransferWriteConditionals>,
        expires_at: Option<OffsetDateTime>,
    ) -> Result<TransferJob, ObjectFormatError> {
        self.ensure_connection_not_removing()?;
        self.enqueue_with_part(
            bucket,
            key,
            content_type,
            body,
            None,
            conditionals,
            expires_at,
        )
        .await
    }

    /// Queue a logical move without copying the encrypted payload. A move is
    /// represented as a new manifest whose chunks point at the original
    /// payload identity, so the normal durable transfer worker only publishes
    /// the small manifest document to Telegram. The caller tombstones the old
    /// manifest after this transfer commits.
    pub(crate) async fn enqueue_manifest_move(
        &self,
        source: &ObjectManifest,
        bucket: &str,
        key: &str,
    ) -> Result<TransferJob, ObjectFormatError> {
        self.ensure_connection_not_removing()?;
        let object_id = Uuid::new_v4();
        let mut manifest = source.clone();
        manifest.schema_version = MANIFEST_SCHEMA_VERSION;
        manifest.commit_state = CommitState::Staging;
        manifest.object_id = object_id;
        manifest.bucket = bucket.to_string();
        manifest.key = key.to_string();
        manifest.version_id = Some(object_id.to_string());
        manifest.telegram = TelegramLocation {
            peer_id: self.storage_chat_id()?,
            message_id: 0,
            document_id: Some(format!("local:{object_id}:manifest")),
        };
        for chunk in &mut manifest.chunks {
            let (source_object_id, source_chunk_order) = chunk.payload_identity(source.object_id);
            chunk.source_object_id = Some(source_object_id);
            chunk.source_chunk_order = Some(source_chunk_order);
        }
        manifest
            .validate()
            .map_err(ObjectFormatError::InvalidPlan)?;

        let id = self.metadata.begin_transfer(object_id, bucket, key)?;
        let dir = self.staging_dir(object_id);
        let queued = async {
            async_fs::create_dir_all(&dir).await?;
            let manifest_bytes = serde_json::to_vec_pretty(&manifest)?;
            self.metadata
                .reserve_staging(&id, manifest_bytes.len() as u64, self.staging_budget)?;
            write_json_file(&dir.join(MANIFEST_FILE_NAME), &manifest)?;
            let operation = self
                .metadata
                .stage_manifest(OperationKind::Put, manifest.clone())?;
            self.metadata.set_transfer_conditionals(
                &id,
                Some(&TransferWriteConditionals {
                    if_match: None,
                    if_none_match: Some("*".to_string()),
                }),
            )?;
            self.metadata
                .queue_transfer(&id, operation, manifest.chunks.len())?;
            self.metadata
                .transfer(&id)?
                .ok_or_else(|| ObjectFormatError::InvalidPlan("move transfer missing".into()))
        }
        .await;
        let job = match queued {
            Ok(job) => job,
            Err(error) => {
                let removed = match async_fs::remove_dir_all(&dir).await {
                    Ok(()) => true,
                    Err(error) if error.kind() == io::ErrorKind::NotFound => true,
                    Err(_) => false,
                };
                let _ = self.metadata.fail_reception(
                    &id,
                    removed,
                    "Metadata-only move could not be durably queued",
                );
                return Err(error);
            }
        };
        self.ensure_workers();
        Ok(job)
    }

    #[allow(clippy::too_many_arguments)]
    pub(super) async fn enqueue_with_part(
        &self,
        bucket: &str,
        key: &str,
        content_type: &str,
        body: Option<StreamingBlob>,
        part: Option<(Uuid, u32, Option<String>)>,
        conditionals: Option<TransferWriteConditionals>,
        expires_at: Option<OffsetDateTime>,
    ) -> Result<TransferJob, ObjectFormatError> {
        self.ensure_connection_not_removing()?;
        let object_id = Uuid::new_v4();
        let chunk_size = self.chunk_size();
        let id = self.metadata.begin_transfer(object_id, bucket, key)?;
        let dir = self.staging_dir(object_id);
        let metadata = Arc::clone(&self.metadata);
        let heartbeat_id = id.clone();
        let (stop_heartbeat, mut heartbeat_stopped) = tokio::sync::watch::channel(false);
        let heartbeat = tokio::spawn(async move {
            loop {
                tokio::select! {
                    changed = heartbeat_stopped.changed() => {
                        if changed.is_err() || *heartbeat_stopped.borrow() { break; }
                    }
                    _ = tokio::time::sleep(std::time::Duration::from_secs(30)) => {
                        if !metadata.renew_receiving(&heartbeat_id).unwrap_or(false) { break; }
                    }
                }
            }
        });

        let received = async {
            if let Some((upload, number, checksum)) = &part {
                self.metadata.with_connection(|c|{c.execute("INSERT INTO multipart_jobs(job_id,upload_id,part_number,expected_checksum) VALUES (?1,?2,?3,?4)",rusqlite::params![id,upload.to_string(),number,checksum])?;Ok(())})?;
            }
            async_fs::create_dir_all(&dir).await?;
            let mut body = body.unwrap_or_else(|| StreamingBlob::new(Body::empty()));
            let mut pending = Vec::with_capacity(chunk_size as usize);
            let mut chunks = Vec::new();
            let mut hasher = Sha256::new();
            let mut offset = 0;
            while let Some(frame) = body.next().await {
                let bytes = frame.map_err(|_| {
                    ObjectFormatError::InvalidPlan(
                        "request body interrupted; resend source file".into(),
                    )
                })?;
                self.add_client_upload_bytes(bytes.len() as u64);
                let mut remaining = bytes.as_ref();
                while !remaining.is_empty() {
                    let take = remaining
                        .len()
                        .min(chunk_size as usize - pending.len());
                    pending.extend_from_slice(&remaining[..take]);
                    remaining = &remaining[take..];
                    if pending.len() == chunk_size as usize {
                        self.stage_transfer_chunk(
                            object_id,
                            &pending,
                            &mut chunks,
                            &mut hasher,
                            &mut offset,
                        )
                        .await?;
                        pending.clear();
                    }
                }
            }
            if !pending.is_empty() {
                self.stage_transfer_chunk(object_id, &pending, &mut chunks, &mut hasher, &mut offset)
                    .await?;
            }
            let manifest = self.new_manifest(ManifestBuildArgs {
                object_id,
                bucket: bucket.into(),
                key: key.into(),
                content_type: content_type.into(),
                expires_at,
                commit_state: CommitState::Staging,
                chunks,
                whole_checksum: hex::encode(hasher.finalize()),
            });
            if let Some((_, _, Some(expected))) = &part
                && *expected != manifest.checksum.whole_object
            {
                return Err(ObjectFormatError::ChecksumMismatch {
                    scope: "multipart part".into(),
                    expected: expected.clone(),
                    actual: manifest.checksum.whole_object.clone(),
                });
            }
            self.finalize_reception(
                &id,
                ManifestBuildArgs {
                    object_id,
                    bucket: bucket.into(),
                    key: key.into(),
                    content_type: content_type.into(),
                    expires_at,
                    commit_state: CommitState::Staging,
                    chunks: manifest.chunks,
                    whole_checksum: manifest.checksum.whole_object,
                },
                conditionals.as_ref(),
            )
        }.await;
        let _ = stop_heartbeat.send(true);
        let _ = heartbeat.await;
        let job = match received {
            Ok(job) => job,
            Err(error) => {
                let removed = match async_fs::remove_dir_all(&dir).await {
                    Ok(()) => true,
                    Err(e) if e.kind() == io::ErrorKind::NotFound => true,
                    Err(_) => false,
                };
                let _ = self.metadata.fail_reception(
                    &id,
                    removed,
                    "Upload reception failed before durable acceptance; resend the source file",
                );
                return Err(error);
            }
        };
        self.ensure_workers();
        Ok(job)
    }

    pub(super) async fn stage_transfer_chunk(
        &self,
        object_id: Uuid,
        bytes: &[u8],
        chunks: &mut Vec<ChunkRef>,
        hasher: &mut Sha256,
        offset: &mut u64,
    ) -> Result<(), ObjectFormatError> {
        let order = u32::try_from(chunks.len())
            .map_err(|_| ObjectFormatError::InvalidPlan("too many chunks".into()))?;
        self.metadata.reserve_staging(
            &object_id.to_string(),
            bytes.len() as u64 + 16,
            self.staging_budget,
        )?;
        let dir = self.staging_dir(object_id);
        let dest = dir.join(chunk_file_name(order));
        let temp = dest.with_extension("tmp");
        let encrypted = self.encrypt_chunk(object_id, order, bytes)?;
        let mut f = async_fs::File::create(&temp).await?;
        f.write_all(&encrypted).await?;
        f.sync_all().await?;
        drop(f);
        async_fs::rename(&temp, &dest).await?;
        sync_directory(&dir)?;
        hasher.update(bytes);
        chunks.push(ChunkRef {
            order,
            offset: *offset,
            size: bytes.len() as u64,
            checksum: sha256_hex(bytes),
            telegram_peer_id: self.storage_chat_id()?,
            telegram_message_id: 0,
            telegram_document_id: Some(format!("local:{object_id}:{order}")),
            source_object_id: None,
            source_chunk_order: None,
            replicas: Vec::new(),
        });
        *offset += bytes.len() as u64;
        Ok(())
    }

    pub(super) fn finalize_reception(
        &self,
        id: &str,
        args: ManifestBuildArgs,
        conditionals: Option<&TransferWriteConditionals>,
    ) -> Result<TransferJob, ObjectFormatError> {
        self.ensure_connection_not_removing()?;
        let dir = self.staging_dir(args.object_id);
        let manifest = self.new_manifest(args);
        write_json_file(&dir.join(MANIFEST_FILE_NAME), &manifest)?;
        let operation = self
            .metadata
            .stage_manifest(OperationKind::Put, manifest.clone())?;
        self.metadata.set_transfer_conditionals(id, conditionals)?;
        self.metadata
            .queue_transfer(id, operation, manifest.chunks.len())?;
        self.metadata
            .transfer(id)?
            .ok_or_else(|| ObjectFormatError::InvalidPlan("queued transfer missing".into()))
    }

    pub fn ensure_workers(&self) {
        if self.worker_runtime.started.swap(true, Ordering::SeqCst) {
            return;
        }
        let (shutdown_tx, shutdown_rx) = tokio::sync::watch::channel(false);
        if let Ok(mut shutdown) = self.worker_runtime.shutdown.lock() {
            *shutdown = Some(shutdown_tx);
        }
        let service = self.clone();
        let mut transfer_shutdown = shutdown_rx.clone();
        let transfer_handle = tokio::spawn(async move {
            let mut last_reconcile =
                tokio::time::Instant::now() - std::time::Duration::from_secs(60);
            loop {
                if *transfer_shutdown.borrow() {
                    break;
                }
                let _ = service
                    .metadata
                    .reclassify_flood_wait_attempts(FALLBACK_FLOOD_WAIT_SECONDS);
                let _ = service.reconcile_unknown_transfers().await;
                match service.metadata.claim_transfer() {
                    Ok(Some(job)) => {
                        let lease = job.lease.clone().unwrap_or_default();
                        let run = service.publish_transfer(&job);
                        tokio::pin!(run);
                        let result = loop {
                            tokio::select! {
                                result = &mut run => break result,
                                _ = tokio::time::sleep(std::time::Duration::from_secs(20)) => {
                                    if !service.metadata.renew_transfer(&job.id,&lease).unwrap_or(false) {
                                        break Err(ObjectFormatError::InvalidPlan("transfer lease lost".into()));
                                    }
                                }
                            }
                        };
                        if let Err(error) = result {
                            let (delay, reason) = match &error {
                                ObjectFormatError::Telegram(_)
                                    if is_explicit_flood_wait(&error) =>
                                {
                                    (
                                        Some(
                                            crate::telegram::retry::parse_flood_wait_seconds(
                                                error.to_string(),
                                            )
                                            .unwrap_or(FALLBACK_FLOOD_WAIT_SECONDS)
                                            .max(1),
                                        ),
                                        "Telegram requested a rate-limit pause; the transfer will resume automatically.",
                                    )
                                }
                                ObjectFormatError::Telegram(_) => (
                                    Some(2u64.saturating_pow(job.attempts.min(8)).max(1)),
                                    "Telegram transfer failed; retry scheduled. Staged data retained.",
                                ),
                                _ => (
                                    None,
                                    "Transfer needs recovery: validate staged files and checksums, then retry or upload the source again.",
                                ),
                            };
                            let _ = service
                                .metadata
                                .transfer_failed(&job.id, &lease, delay, reason);
                        }
                    }
                    Ok(None) => tokio::select! {
                        _ = tokio::time::sleep(std::time::Duration::from_millis(200)) => {},
                        _ = transfer_shutdown.changed() => {},
                    },
                    Err(_) => tokio::select! {
                        _ = tokio::time::sleep(std::time::Duration::from_secs(2)) => {},
                        _ = transfer_shutdown.changed() => {},
                    },
                }
                if last_reconcile.elapsed().as_secs() >= 60 {
                    // Never scan remote committed payloads on the request path.
                    let _ = service.import_legacy_staging();
                    let _ = service.cleanup_completed_staging().await;
                    last_reconcile = tokio::time::Instant::now();
                }
            }
        });
        let service = self.clone();
        let mut replication_shutdown = shutdown_rx.clone();
        let replication_handle = tokio::spawn(async move {
            loop {
                if *replication_shutdown.borrow() {
                    break;
                }
                match service.metadata.claim_replication_job() {
                    Ok(Some(job)) => {
                        if let Err(error) = service.process_replication_job(&job).await {
                            let _ = service.metadata.finish_replication(
                                &job.id,
                                "failed",
                                Some(&error.to_string()),
                                job.mode == "automatic",
                            );
                        }
                    }
                    Ok(None) => tokio::select! {
                        _ = tokio::time::sleep(std::time::Duration::from_millis(500)) => {},
                        _ = replication_shutdown.changed() => {},
                    },
                    Err(_) => tokio::select! {
                        _ = tokio::time::sleep(std::time::Duration::from_secs(2)) => {},
                        _ = replication_shutdown.changed() => {},
                    },
                }
            }
        });
        let service = self.clone();
        let mut rechunk_shutdown = shutdown_rx.clone();
        let rechunk_handle = tokio::spawn(async move {
            loop {
                if *rechunk_shutdown.borrow() {
                    break;
                }
                match service.metadata.claim_rechunk_job() {
                    Ok(Some(job)) => {
                        if let Err(error) = service.process_rechunk_job(&job).await {
                            let _ = service.metadata.finish_rechunk(
                                &job.id,
                                "failed",
                                Some(&error.to_string()),
                            );
                        }
                    }
                    Ok(None) => tokio::select! {
                        _ = tokio::time::sleep(std::time::Duration::from_millis(300)) => {},
                        _ = rechunk_shutdown.changed() => {},
                    },
                    Err(_) => tokio::select! {
                        _ = tokio::time::sleep(std::time::Duration::from_secs(2)) => {},
                        _ = rechunk_shutdown.changed() => {},
                    },
                }
            }
        });
        let service = self.clone();
        let mut recovery_shutdown = shutdown_rx.clone();
        let recovery_wake = Arc::clone(&self.worker_runtime.recovery_wake);
        let recovery_handle = tokio::spawn(async move {
            if service.recovery_verifier_enabled() && service.recovery_verify_startup() {
                let _ = service.refresh_recovery_snapshot().await;
            } else if service.recovery_verifier_enabled() {
                service.schedule_recovery_verification();
            }
            loop {
                if *recovery_shutdown.borrow() {
                    break;
                }
                if !service.recovery_verifier_enabled() {
                    tokio::select! {
                        _ = tokio::time::sleep(std::time::Duration::from_secs(1)) => {},
                        _ = recovery_wake.notified() => {},
                        _ = recovery_shutdown.changed() => {}
                    }
                    continue;
                }
                tokio::select! {
                    _ = tokio::time::sleep(std::time::Duration::from_secs(service.recovery_verify_interval_secs())) => {
                        if service.recovery_verifier_enabled() {
                            let _ = service.refresh_recovery_snapshot().await;
                        }
                    }
                    _ = recovery_wake.notified() => {
                        if service.recovery_verifier_enabled() {
                            let _ = service.refresh_recovery_snapshot().await;
                        }
                    }
                    _ = recovery_shutdown.changed() => {}
                }
            }
        });
        let service = self.clone();
        let mut cleanup_shutdown = shutdown_rx;
        let cleanup_wake = Arc::clone(&self.worker_runtime.cleanup_wake);
        let cleanup_handle = tokio::spawn(async move {
            loop {
                if *cleanup_shutdown.borrow() {
                    break;
                }
                let _ = service.reconcile_cleanup_evidence().await;
                let _ = service.finalize_connection_removal().await;
                let _ = service
                    .metadata
                    .tombstone_expired_active_manifests(time::OffsetDateTime::now_utc());
                match service.metadata.claim_cleanup() {
                    Ok(Some(target)) => {
                        if let Err(error) = service.process_cleanup_target(&target).await {
                            let delay =
                                crate::telegram::retry::parse_flood_wait_seconds(error.to_string())
                                    .unwrap_or(30)
                                    .max(1);
                            let _ = service.metadata.fail_cleanup(
                                target.id,
                                &target.lease,
                                delay,
                                "Cleanup failed; durable retry scheduled",
                            );
                        }
                        let _ = service.finalize_connection_removal().await;
                    }
                    Ok(None) => {
                        let delay = service.metadata.cleanup_wake_delay_secs().unwrap_or(60);
                        tokio::select! {
                        _ = tokio::time::sleep(std::time::Duration::from_secs(delay)) => {},
                        _ = cleanup_wake.notified() => {},
                        _ = cleanup_shutdown.changed() => {},
                        }
                    }
                    Err(_) => tokio::select! {
                        _ = tokio::time::sleep(std::time::Duration::from_secs(5)) => {},
                        _ = cleanup_wake.notified() => {},
                        _ = cleanup_shutdown.changed() => {},
                    },
                }
            }
        });
        if let Ok(mut handles) = self.worker_runtime.handles.lock() {
            handles.push(transfer_handle);
            handles.push(replication_handle);
            handles.push(rechunk_handle);
            handles.push(recovery_handle);
            handles.push(cleanup_handle);
        }
    }

    /// Re-chunk one committed object through the normal durable upload path.
    /// The old manifest remains recoverable until the replacement is committed,
    /// while the metadata lock makes reads return a clear temporary-unavailable
    /// response instead of serving a mixed chunk layout.
    async fn process_rechunk_job(
        &self,
        job: &crate::metadata::RechunkJob,
    ) -> Result<(), ObjectFormatError> {
        let old_id = Uuid::parse_str(&job.object_id)
            .map_err(|error| ObjectFormatError::InvalidPlan(error.to_string()))?;
        let old_manifest = self.metadata.get_manifest(old_id)?.ok_or_else(|| {
            ObjectFormatError::InvalidRead(format!(
                "re-chunk source object not found: {}",
                job.object_id
            ))
        })?;
        let new_size = usize::try_from(job.new_chunk_size).map_err(|_| {
            ObjectFormatError::InvalidPlan("new chunk size is too large for this platform".into())
        })?;
        if new_size == 0 {
            return Err(ObjectFormatError::InvalidPlan(
                "new chunk size must be non-zero".into(),
            ));
        }
        let planned_total = Self::plan_chunks(old_manifest.content_length, job.new_chunk_size)?
            .chunks
            .len() as u64;
        self.metadata
            .update_rechunk_progress(&job.id, planned_total, 0, 0)?;
        let new_id = Uuid::new_v4();
        let dir = self.staging_dir(new_id);
        async_fs::create_dir_all(&dir).await?;
        let mut pending = Vec::with_capacity(new_size.min(16 * 1024 * 1024));
        let mut chunks = Vec::new();
        let mut hasher = Sha256::new();
        let mut offset = 0_u64;
        let mut bytes_done = 0_u64;
        for source in &old_manifest.chunks {
            let plaintext = self.read_manifest_chunk(&old_manifest, source).await?;
            bytes_done = bytes_done.saturating_add(plaintext.len() as u64);
            hasher.update(&plaintext);
            pending.extend_from_slice(&plaintext);
            while pending.len() >= new_size {
                let remainder = pending.split_off(new_size);
                self.stage_transfer_chunk(
                    new_id,
                    &pending,
                    &mut chunks,
                    &mut Sha256::new(),
                    &mut offset,
                )
                .await?;
                pending = remainder;
                self.metadata.update_rechunk_progress(
                    &job.id,
                    planned_total,
                    chunks.len() as u64,
                    bytes_done,
                )?;
            }
        }
        if !pending.is_empty() {
            self.stage_transfer_chunk(
                new_id,
                &pending,
                &mut chunks,
                &mut Sha256::new(),
                &mut offset,
            )
            .await?;
        }
        let manifest_args = ManifestBuildArgs {
            object_id: new_id,
            bucket: old_manifest.bucket.clone(),
            key: old_manifest.key.clone(),
            content_type: old_manifest.content_type.clone(),
            expires_at: old_manifest.expires_at,
            commit_state: CommitState::Staging,
            chunks,
            whole_checksum: hex::encode(hasher.finalize()),
        };
        let transfer_id =
            self.metadata
                .begin_transfer(new_id, &old_manifest.bucket, &old_manifest.key)?;
        let transfer = self.finalize_reception(&transfer_id, manifest_args, None)?;
        let _replacement = self.wait_transfer(&transfer.id).await?;
        if job.apply_to_replicas {
            for target in &job.replica_targets {
                if target.account_id == job.source_account_id {
                    continue;
                }
                self.metadata.queue_replication(
                    &job.source_account_id,
                    &target.account_id,
                    &job.bucket,
                    std::slice::from_ref(&job.key),
                    "one_time",
                    &target.access_mode,
                )?;
            }
        }
        self.metadata
            .tombstone_manifest(old_id, "replaced by re-chunking")?;
        self.metadata.finish_rechunk(&job.id, "completed", None)?;
        let _ = async_fs::remove_dir_all(dir).await;
        Ok(())
    }

    async fn process_replication_job(
        &self,
        job: &crate::metadata::ReplicationJob,
    ) -> Result<(), ObjectFormatError> {
        let active = self
            .metadata
            .active_connection_id()?
            .unwrap_or_else(|| "legacy".to_string());
        let source_manager = if job.source_account_id == active
            || (job.source_account_id == "legacy" && active == "legacy")
        {
            Some(Arc::clone(&self.transport_manager))
        } else {
            None
        };
        let mut manifests = self.list_bucket_manifests(&job.bucket, None)?;
        if !job.object_keys.is_empty() {
            let selected = job
                .object_keys
                .iter()
                .collect::<std::collections::HashSet<_>>();
            manifests.retain(|manifest| selected.contains(&manifest.key));
        }
        let objects_total = manifests.len() as u64;
        let chunks_total = manifests
            .iter()
            .map(|manifest| manifest.chunks.len() as u64)
            .sum();
        self.metadata
            .update_replication_progress(&job.id, objects_total, 0, chunks_total, 0, 0)?;
        let target_manager = if job.access_mode == "replica" {
            Some(self.account_manager(&job.target_account_id).await?)
        } else {
            None
        };
        let target = if let Some(manager) = &target_manager {
            Some(manager.current().await?)
        } else {
            None
        };
        let target_storage_chat_id = if let Some(target) = &target {
            Some(target.status().await?.storage_chat_id)
        } else {
            None
        };
        // A same-chat replica only needs the target account to be available:
        // it can reference the existing Telegram message directly. Resolve
        // the source transport lazily so a disconnected source account does
        // not block that metadata-only path. Different chats still acquire
        // it before the first payload copy.
        let mut source_transport: Option<Arc<crate::telegram::TelegramTransport>> = None;
        let mut done_objects = 0_u64;
        let mut done_chunks = 0_u64;
        let mut bytes_done = 0_u64;
        for mut manifest in manifests {
            for chunk in &mut manifest.chunks {
                let already_present = chunk.replicas.iter().any(|replica| {
                    replica.account_id == job.target_account_id
                        && ((job.access_mode == "access" && replica.mode == ReplicaMode::Access)
                            || (job.access_mode == "replica"
                                && replica.mode == ReplicaMode::Replica))
                });
                if already_present {
                    done_chunks += 1;
                    self.metadata.update_replication_progress(
                        &job.id,
                        objects_total,
                        done_objects,
                        chunks_total,
                        done_chunks,
                        bytes_done,
                    )?;
                    continue;
                }
                let location = if job.access_mode == "access" {
                    self.metadata.insert_replica_location(
                        manifest.object_id,
                        chunk.order,
                        &job.target_account_id,
                        "access",
                        &chunk.telegram_peer_id,
                        chunk.telegram_message_id,
                        chunk.telegram_document_id.as_deref(),
                    )?;
                    TelegramLocation {
                        peer_id: chunk.telegram_peer_id.clone(),
                        message_id: chunk.telegram_message_id,
                        document_id: chunk.telegram_document_id.clone(),
                    }
                } else {
                    let source_location = if job.source_account_id == active
                        || (job.source_account_id == "legacy" && active == "legacy")
                    {
                        Some(TelegramLocation {
                            peer_id: chunk.telegram_peer_id.clone(),
                            message_id: chunk.telegram_message_id,
                            document_id: chunk.telegram_document_id.clone(),
                        })
                    } else {
                        chunk
                            .replicas
                            .iter()
                            .find(|replica| {
                                replica.account_id == job.source_account_id
                                    && replica.mode == ReplicaMode::Replica
                            })
                            .map(|replica| TelegramLocation {
                                peer_id: replica.telegram_peer_id.clone(),
                                message_id: replica.telegram_message_id,
                                document_id: replica.telegram_document_id.clone(),
                            })
                    };
                    let source_location = source_location.ok_or_else(|| {
                        ObjectFormatError::InvalidPlan(format!(
                            "source account {} has no physical replica for object {} chunk {}",
                            job.source_account_id, manifest.key, chunk.order
                        ))
                    })?;
                    let location = if can_reuse_replica_message(
                        &source_location.peer_id,
                        target_storage_chat_id.as_deref(),
                    ) {
                        // Both connected accounts point at the same Telegram
                        // storage chat. The target account can therefore use
                        // the existing message without a server-side download
                        // or a second Telegram upload.
                        source_location
                    } else {
                        let source_transport = match &source_transport {
                            Some(transport) => Arc::clone(transport),
                            None => {
                                let manager = if let Some(manager) = source_manager.as_ref() {
                                    Arc::clone(manager)
                                } else {
                                    self.account_manager(&job.source_account_id).await?
                                };
                                let transport = manager.current().await?;
                                source_transport = Some(Arc::clone(&transport));
                                transport
                            }
                        };
                        let message_id =
                            i32::try_from(source_location.message_id).map_err(|_| {
                                ObjectFormatError::InvalidRead(
                                    "source message id is out of range".into(),
                                )
                            })?;
                        let ciphertext = self
                            .download_message_bytes_once_with_transport(
                                Arc::clone(&source_transport),
                                message_id,
                            )
                            .await?;
                        bytes_done = bytes_done.saturating_add(ciphertext.len() as u64);
                        self.upload_replica_bytes(
                            target.as_ref().ok_or_else(|| {
                                ObjectFormatError::InvalidPlan(
                                    "target transport is unavailable".into(),
                                )
                            })?,
                            &job.id,
                            chunk.order,
                            &ciphertext,
                            &job.target_account_id,
                        )
                        .await?
                    };
                    self.metadata.insert_replica_location(
                        manifest.object_id,
                        chunk.order,
                        &job.target_account_id,
                        "replica",
                        &location.peer_id,
                        location.message_id,
                        location.document_id.as_deref(),
                    )?;
                    chunk
                        .replicas
                        .retain(|replica| replica.account_id != job.target_account_id);
                    chunk.replicas.push(ChunkReplica {
                        account_id: job.target_account_id.clone(),
                        mode: ReplicaMode::Replica,
                        chunk_size: chunk.size,
                        telegram_peer_id: location.peer_id.clone(),
                        telegram_message_id: location.message_id,
                        telegram_document_id: location.document_id.clone(),
                    });
                    location
                };
                if job.access_mode == "access" {
                    chunk
                        .replicas
                        .retain(|replica| replica.account_id != job.target_account_id);
                    chunk.replicas.push(ChunkReplica {
                        account_id: job.target_account_id.clone(),
                        mode: ReplicaMode::Access,
                        chunk_size: chunk.size,
                        telegram_peer_id: location.peer_id.clone(),
                        telegram_message_id: location.message_id,
                        telegram_document_id: location.document_id.clone(),
                    });
                }
                done_chunks += 1;
                self.metadata.update_replication_progress(
                    &job.id,
                    objects_total,
                    done_objects,
                    chunks_total,
                    done_chunks,
                    bytes_done,
                )?;
            }
            self.metadata.update_manifest(manifest)?;
            done_objects += 1;
            self.metadata.update_replication_progress(
                &job.id,
                objects_total,
                done_objects,
                chunks_total,
                done_chunks,
                bytes_done,
            )?;
        }
        self.metadata.finish_replication(
            &job.id,
            if job.mode == "automatic" {
                "scheduled"
            } else {
                "completed"
            },
            None,
            job.mode == "automatic",
        )?;
        Ok(())
    }

    pub(crate) async fn account_manager(
        &self,
        account_id: &str,
    ) -> Result<Arc<TelegramTransportManager>, ObjectFormatError> {
        if let Ok(managers) = self.account_managers.lock()
            && let Some(manager) = managers.get(account_id)
        {
            return Ok(Arc::clone(manager));
        }
        let (_, settings) = self.metadata.telegram_account(account_id)?.ok_or_else(|| {
            ObjectFormatError::InvalidPlan("target Telegram account is not configured".into())
        })?;
        let config = self.config.clone().ok_or_else(|| {
            ObjectFormatError::InvalidPlan(
                "account transports are unavailable in this service instance".into(),
            )
        })?;
        let api_id = settings.telegram_api_id.ok_or_else(|| {
            ObjectFormatError::InvalidPlan("target account API ID is missing".into())
        })?;
        let api_hash = settings.telegram_api_hash.ok_or_else(|| {
            ObjectFormatError::InvalidPlan("target account API hash is missing".into())
        })?;
        let storage_chat_id = crate::config::normalize_telegram_storage_chat_id(
            settings.telegram_storage_chat_id.as_deref(),
        )
        .map_err(|error| ObjectFormatError::InvalidPlan(error.to_string()))?;
        let session_path = settings
            .telegram_session_path
            .map(PathBuf::from)
            .unwrap_or_else(|| self.data_dir.join(format!("telegram-{account_id}.session")));
        let bootstrap = crate::config::ResolvedTelegramBootstrap {
            telegram_api_id: api_id,
            telegram_api_hash: api_hash,
            telegram_session_path: session_path,
            telegram_storage_chat_id: storage_chat_id,
            telegram_proxy_url: settings.telegram_proxy_url,
            telegram_proxy_username: settings.telegram_proxy_username,
            telegram_proxy_password: settings.telegram_proxy_password,
            telegram_proxy_mode: settings
                .telegram_proxy_mode
                .unwrap_or_else(|| "auto".into()),
        };
        let manager = TelegramTransportManager::open_for_bootstrap(config, bootstrap).await?;
        // Prime the first health result once, then keep it fresh in the
        // background. A failed prime is left for the monitor to classify as a
        // disconnected account without blocking the caller indefinitely.
        let _ = manager.refresh().await;
        manager.start_health_monitor();
        if let Ok(mut managers) = self.account_managers.lock() {
            if let Some(existing) = managers.get(account_id) {
                return Ok(Arc::clone(existing));
            }
            managers.insert(account_id.to_string(), Arc::clone(&manager));
        }
        Ok(manager)
    }

    pub(crate) async fn upload_replica_bytes(
        &self,
        transport: &Arc<crate::telegram::TelegramTransport>,
        job_id: &str,
        order: u32,
        bytes: &[u8],
        account_id: &str,
    ) -> Result<TelegramLocation, ObjectFormatError> {
        if transport.is_mock() {
            let message_id = self.next_mock_message_id()?;
            let dir = self.mock_telegram_dir();
            fs::create_dir_all(&dir)?;
            fs::write(dir.join(format!("{message_id}.bin")), bytes)?;
            fs::write(
                dir.join(format!("{message_id}.json")),
                serde_json::to_vec(
                    &serde_json::json!({"replica_job":job_id,"account_id":account_id,"chunk":order}),
                )?,
            )?;
            self.add_telegram_upload_bytes(bytes.len() as u64);
            return Ok(TelegramLocation {
                peer_id: transport.status().await?.storage_chat_id,
                message_id: i64::from(message_id),
                document_id: Some(format!("mock-replica:{account_id}:{message_id}")),
            });
        }
        let dir = self.data_dir.join("replica-staging").join(job_id);
        fs::create_dir_all(&dir)?;
        let path = dir.join(format!("{order}.bin"));
        fs::write(&path, bytes)?;
        let location = transport
            .upload_path(&path, &format!("replica-{account_id}-{order}.bin"))
            .await?;
        self.add_telegram_upload_bytes(bytes.len() as u64);
        let _ = fs::remove_file(path);
        Ok(location)
    }

    async fn read_manifest_chunk(
        &self,
        manifest: &ObjectManifest,
        chunk: &ChunkRef,
    ) -> Result<Vec<u8>, ObjectFormatError> {
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
        let actual = sha256_hex(&plaintext);
        if actual != chunk.checksum {
            return Err(ObjectFormatError::ChecksumMismatch {
                scope: format!("chunk {}", chunk.order),
                expected: chunk.checksum.clone(),
                actual,
            });
        }
        Ok(plaintext)
    }

    async fn finalize_connection_removal(&self) -> Result<(), ObjectFormatError> {
        let Some(job) = self.metadata.connection_removal_job()? else {
            return Ok(());
        };
        if job.state == "remote_cleanup_pending" {
            if !self.metadata.connection_removal_cleanup_pending(&job.id)? {
                self.detach_connection_if_owned(&job).await?;
                self.metadata
                    .purge_connection_operations(&job.connection_id)?;
                self.metadata
                    .finish_connection_removal(&job.id, "completed", None)?;
            }
            return Ok(());
        }
        if !matches!(job.state.as_str(), "pending" | "running") {
            return Ok(());
        }

        for object_id in self.metadata.connection_removal_objects(&job.id)? {
            if let Ok(object_id) = Uuid::parse_str(&object_id) {
                match fs::remove_file(self.manifest_path(object_id)) {
                    Ok(()) => {}
                    Err(error) if error.kind() == io::ErrorKind::NotFound => {}
                    Err(error) => return Err(error.into()),
                }
                match fs::remove_dir_all(self.chunk_dir(object_id)) {
                    Ok(()) => {}
                    Err(error) if error.kind() == io::ErrorKind::NotFound => {}
                    Err(error) => return Err(error.into()),
                }
            }
        }
        let next_state = if job.delete_uploaded_files
            && self.metadata.connection_removal_cleanup_pending(&job.id)?
        {
            "remote_cleanup_pending"
        } else {
            self.detach_connection_if_owned(&job).await?;
            self.metadata
                .purge_connection_operations(&job.connection_id)?;
            "completed"
        };
        self.metadata
            .finish_connection_removal(&job.id, next_state, None)?;
        Ok(())
    }

    async fn detach_connection_if_owned(
        &self,
        job: &crate::metadata::ConnectionRemovalJob,
    ) -> Result<(), ObjectFormatError> {
        let active_connection_matches = match self.metadata.active_connection_id()? {
            Some(active) => active == job.connection_id,
            None => {
                job.connection_id == "legacy"
                    && self.metadata.telegram_bootstrap_settings()?.is_some()
            }
        };
        if active_connection_matches {
            self.metadata.clear_telegram_bootstrap_settings()?;
            self.metadata.clear_active_connection_id()?;
            self.set_storage_chat_id(String::new());
            self.transport_manager.disconnect().await;
            // The removal job owns the connection generation. Once local
            // visibility and any requested remote cleanup are complete, drop
            // its account registry row as well so overview health cannot keep
            // reporting a removed transport as connected.
            let _ = self.metadata.delete_telegram_account(&job.connection_id)?;
            self.invalidate_account_manager(&job.connection_id);
        }
        Ok(())
    }

    pub async fn shutdown_workers(&self) {
        if let Ok(shutdown) = self.worker_runtime.shutdown.lock()
            && let Some(sender) = shutdown.as_ref()
        {
            let _ = sender.send(true);
        }
        let handles = self
            .worker_runtime
            .handles
            .lock()
            .map(|mut handles| std::mem::take(&mut *handles))
            .unwrap_or_default();
        for handle in handles {
            let _ = handle.await;
        }
        if let Ok(mut shutdown) = self.worker_runtime.shutdown.lock() {
            *shutdown = None;
        }
        self.worker_runtime.started.store(false, Ordering::SeqCst);
    }

    pub(crate) fn notify_cleanup_worker(&self) {
        self.worker_runtime.cleanup_wake.notify_one();
    }

    pub async fn wait_transfer(&self, id: &str) -> Result<ObjectManifest, ObjectFormatError> {
        self.ensure_workers();
        loop {
            let job = self
                .metadata
                .transfer(id)?
                .ok_or_else(|| ObjectFormatError::InvalidPlan("transfer missing".into()))?;
            match job.state.as_str() {
                "completed" | "cleaned" => {
                    if let Some((upload, number)) = self.metadata.multipart_job(&job.id)?
                        && number > 0
                    {
                        return self
                            .metadata
                            .get_multipart_part(
                                Uuid::parse_str(&upload)
                                    .map_err(|e| ObjectFormatError::InvalidPlan(e.to_string()))?,
                                number,
                            )?
                            .and_then(|part| part.manifest)
                            .ok_or_else(|| {
                                ObjectFormatError::InvalidPlan(
                                    "completed multipart part manifest missing".into(),
                                )
                            });
                    }
                    return self
                        .metadata
                        .get_manifest(
                            Uuid::parse_str(&job.object_id)
                                .map_err(|e| ObjectFormatError::InvalidPlan(e.to_string()))?,
                        )?
                        .ok_or_else(|| {
                            ObjectFormatError::InvalidPlan("completed manifest missing".into())
                        });
                }
                "superseded" => {
                    // A duplicate multipart request may be waiting while the
                    // canonical job finishes. Return that published part if
                    // it exists instead of leaving the caller to retry again.
                    if let Some((upload, number)) = self.metadata.multipart_job(&job.id)?
                        && number > 0
                        && let Some(part) = self.metadata.get_multipart_part(
                            Uuid::parse_str(&upload)
                                .map_err(|e| ObjectFormatError::InvalidPlan(e.to_string()))?,
                            number,
                        )?
                        && let Some(manifest) = part.manifest
                    {
                        return Ok(manifest);
                    }
                    return Err(ObjectFormatError::InvalidPlan(
                        job.error
                            .unwrap_or_else(|| "transfer was superseded".into()),
                    ));
                }
                "cancelled" | "recovery_required" | "reception_failed" => {
                    return Err(ObjectFormatError::InvalidPlan(
                        job.error
                            .unwrap_or_else(|| "transfer needs recovery".into()),
                    ));
                }
                _ => tokio::time::sleep(std::time::Duration::from_millis(50)).await,
            }
        }
    }

    async fn publish_transfer(&self, job: &TransferJob) -> Result<(), ObjectFormatError> {
        let object_id = Uuid::parse_str(&job.object_id)
            .map_err(|e| ObjectFormatError::InvalidPlan(e.to_string()))?;
        let operation = Uuid::parse_str(job.operation_id.as_deref().unwrap_or(""))
            .map_err(|e| ObjectFormatError::InvalidPlan(e.to_string()))?;
        let mut manifest = self
            .metadata
            .get_manifest(object_id)?
            .ok_or_else(|| ObjectFormatError::InvalidPlan("staged manifest missing".into()))?;
        let dir = self.staging_dir(operation);
        let lease = job.lease.as_deref().unwrap_or("");
        let pending =
            futures::stream::iter(manifest.chunks.clone().into_iter().map(|mut chunk| {
                let dir = &dir;
                async move {
                    if chunk.references_remote_payload() {
                        if chunk.telegram_peer_id.trim().is_empty()
                            || chunk.telegram_message_id <= 0
                        {
                            return Err(ObjectFormatError::InvalidPlan(format!(
                                "composed chunk {} has no durable Telegram location",
                                chunk.order
                            )));
                        }
                        return Ok::<_, ObjectFormatError>(chunk);
                    }
                    let location = match self.metadata.checkpoint_location(&job.id, chunk.order)? {
                        Some(location) => location,
                        None => {
                            let path = dir.join(chunk_file_name(chunk.order));
                            let encrypted = async_fs::read(&path).await?;
                            let plain = self.decrypt_chunk(object_id, chunk.order, &encrypted)?;
                            let actual = sha256_hex(&plain);
                            if actual != chunk.checksum {
                                return Err(ObjectFormatError::ChecksumMismatch {
                                    scope: format!("chunk {}", chunk.order),
                                    expected: chunk.checksum.clone(),
                                    actual,
                                });
                            }
                            drop(plain);
                            drop(encrypted);
                            let attempt =
                                self.metadata
                                    .begin_send_attempt(&job.id, lease, chunk.order)?;
                            let location = match self
                                .upload_local_file_to_telegram(&path, &attempt.token)
                                .await
                            {
                                Ok(location) => location,
                                Err(error) => {
                                    let _ = if is_explicit_flood_wait(&error) {
                                        self.metadata.mark_send_attempt_retryable(
                                            &job.id,
                                            lease,
                                            chunk.order,
                                            &attempt.id,
                                            "telegram_flood_wait",
                                            &error.to_string(),
                                        )
                                    } else {
                                        self.metadata.mark_send_attempt_unknown(
                                            &job.id,
                                            lease,
                                            chunk.order,
                                            &attempt.id,
                                            "telegram_send",
                                            &error.to_string(),
                                        )
                                    };
                                    return Err(error);
                                }
                            };
                            self.metadata.finish_send_attempt(
                                &job.id,
                                lease,
                                chunk.order,
                                &attempt.id,
                                &location,
                            )?;
                            location
                        }
                    };
                    chunk.telegram_peer_id = location.peer_id;
                    chunk.telegram_message_id = location.message_id;
                    chunk.telegram_document_id = location.document_id;
                    Ok::<_, ObjectFormatError>(chunk)
                }
            }))
            // `try_collect` cancels sibling futures on the first error. A
            // cancelled Telegram send has an indeterminate acknowledgement, so
            // publish chunks one at a time and keep every attempt recoverable.
            .buffered(1);
        let mut results: Vec<ChunkRef> = futures::TryStreamExt::try_collect(pending).await?;
        results.sort_by_key(|c| c.order);
        manifest.chunks = results;
        // The publication fence cannot be cancelled halfway through remote manifest publication.
        self.metadata.with_connection(|c| {
            if c.execute("UPDATE transfer_jobs SET state='committing' WHERE id=?1 AND lease=?2 AND state='uploading' AND lease_until>=?3",rusqlite::params![job.id,lease,crate::durable::now()])? != 1 {
                return Err(MetadataError::InvalidManifest("publication fence lost".into()));
            } Ok(())
        })?;
        manifest.commit_state = CommitState::Committed;
        let path = dir.join(MANIFEST_FILE_NAME);
        write_json_file(&path, &manifest)?;
        manifest.telegram = match self.metadata.checkpoint_location(&job.id, u32::MAX)? {
            Some(location) => location,
            None => {
                let attempt = self.metadata.begin_send_attempt(&job.id, lease, u32::MAX)?;
                let location = match self
                    .upload_local_file_to_telegram(&path, &attempt.token)
                    .await
                {
                    Ok(location) => location,
                    Err(error) => {
                        let _ = if is_explicit_flood_wait(&error) {
                            self.metadata.mark_send_attempt_retryable(
                                &job.id,
                                lease,
                                u32::MAX,
                                &attempt.id,
                                "telegram_flood_wait",
                                &error.to_string(),
                            )
                        } else {
                            self.metadata.mark_send_attempt_unknown(
                                &job.id,
                                lease,
                                u32::MAX,
                                &attempt.id,
                                "telegram_send",
                                &error.to_string(),
                            )
                        };
                        return Err(error);
                    }
                };
                self.metadata.finish_send_attempt(
                    &job.id,
                    lease,
                    u32::MAX,
                    &attempt.id,
                    &location,
                )?;
                location
            }
        };
        manifest.commit_state = CommitState::Staging;
        if let Some((upload, number)) = self.metadata.multipart_job(&job.id)? {
            if number == 0 {
                self.metadata
                    .commit_transfer_manifest(operation, &job.id, lease, manifest)?;
            } else {
                self.metadata.update_manifest(manifest)?;
                self.metadata
                    .finish_part_job(&job.id, lease, &upload, number)?;
            }
        } else {
            self.metadata
                .commit_transfer_manifest(operation, &job.id, lease, manifest)?;
        }
        // Cleanup failure cannot change a committed upload into an HTTP failure.
        let _ = self.cleanup_completed_staging().await;
        Ok(())
    }

    async fn reconcile_unknown_transfers(&self) -> Result<(), ObjectFormatError> {
        for job in self
            .metadata
            .transfers(100, 0)?
            .into_iter()
            .filter(|job| job.state == "recovery_required")
        {
            let attempts = self.metadata.unresolved_send_attempts(&job.id)?;
            if attempts.is_empty() {
                continue;
            }
            let operation = Uuid::parse_str(job.operation_id.as_deref().unwrap_or(""))
                .map_err(|error| ObjectFormatError::InvalidPlan(error.to_string()))?;
            let directory = self.staging_dir(operation);
            let mut all_resolved = true;
            for attempt in attempts {
                let path = if attempt.order == u32::MAX {
                    directory.join(MANIFEST_FILE_NAME)
                } else {
                    directory.join(chunk_file_name(attempt.order))
                };
                if !path.exists() {
                    all_resolved = false;
                    continue;
                }
                match self
                    .reconcile_remote_file(&path, &attempt.token, attempt.started_at)
                    .await
                {
                    Ok(RemoteReconciliation::Match(location)) => {
                        self.metadata
                            .resolve_send_attempt(&attempt, Some(&location), None)?;
                    }
                    Ok(RemoteReconciliation::Absent) => {
                        self.metadata.resolve_send_attempt(
                            &attempt,
                            None,
                            Some("No matching document was found after scanning messages newer than the attempt"),
                        )?;
                    }
                    Err(error) => {
                        all_resolved = false;
                        let _ = self.metadata.set_transfer_error(
                            &job.id,
                            "Automatic reconciliation is waiting for Telegram connectivity",
                        );
                        tracing::warn!(job_id = %job.id, error = %error, "transfer reconciliation deferred");
                    }
                }
            }
            if all_resolved {
                let _ = self.metadata.queue_after_reconciliation(&job.id)?;
            }
        }
        Ok(())
    }

    pub(crate) async fn reconcile_remote_file(
        &self,
        path: &std::path::Path,
        token: &str,
        started_at: i64,
    ) -> Result<RemoteReconciliation, ObjectFormatError> {
        let transport = self.transport_manager.current().await?;
        if transport.is_mock() {
            let directory = self.mock_telegram_dir();
            for entry in fs::read_dir(&directory)? {
                let entry = entry?;
                if entry.path().extension().and_then(|value| value.to_str()) != Some("json") {
                    continue;
                }
                let details: serde_json::Value = serde_json::from_slice(&fs::read(entry.path())?)?;
                if details.get("file_name").and_then(serde_json::Value::as_str) != Some(token) {
                    continue;
                }
                let message_id = entry
                    .path()
                    .file_stem()
                    .and_then(|value| value.to_str())
                    .and_then(|value| value.parse::<i32>().ok())
                    .ok_or_else(|| {
                        ObjectFormatError::InvalidPlan("invalid mock message id".into())
                    })?;
                if fs::read(path)? != fs::read(directory.join(format!("{message_id}.bin")))? {
                    return Err(ObjectFormatError::InvalidPlan(
                        "Telegram reconciliation found a token collision with different bytes"
                            .into(),
                    ));
                }
                return Ok(RemoteReconciliation::Match(TelegramLocation {
                    peer_id: self.storage_chat_id()?,
                    message_id: i64::from(message_id),
                    document_id: Some(format!("mock:{message_id}:{token}")),
                }));
            }
            return Ok(RemoteReconciliation::Absent);
        }
        let client = transport.client()?;
        let storage_peer = transport.storage_peer().await?;
        let mut messages = client.iter_messages(storage_peer).limit(1000);
        let mut scanned = 0_usize;
        let mut reached_attempt_boundary = false;
        while let Some(message) = messages.next().await.map_err(|error| {
            ObjectFormatError::Telegram(crate::telegram::TelegramTransportError::Rpc(
                error.to_string(),
            ))
        })? {
            scanned += 1;
            if message.date().timestamp() < started_at {
                reached_attempt_boundary = true;
                break;
            }
            let Some(Media::Document(document)) = message.media() else {
                continue;
            };
            if document.name() != Some(token) {
                continue;
            }
            let mut download = client.iter_download(&Media::Document(document.clone()));
            let mut remote = Vec::new();
            while let Some(chunk) = download.next().await.map_err(|error| {
                ObjectFormatError::Telegram(crate::telegram::TelegramTransportError::Rpc(
                    error.to_string(),
                ))
            })? {
                remote.extend_from_slice(&chunk);
            }
            if fs::read(path)? != remote {
                return Err(ObjectFormatError::InvalidPlan(
                    "Telegram reconciliation found a token collision with different bytes".into(),
                ));
            }
            return Ok(RemoteReconciliation::Match(TelegramLocation {
                peer_id: self.storage_chat_id()?,
                message_id: i64::from(message.id()),
                document_id: Some(document.id().to_string()),
            }));
        }
        if !reached_attempt_boundary && scanned >= 1000 {
            return Err(ObjectFormatError::InvalidPlan(
                "Telegram reconciliation scan limit reached before the attempt boundary".into(),
            ));
        }
        Ok(RemoteReconciliation::Absent)
    }

    fn import_legacy_staging(&self) -> Result<(), ObjectFormatError> {
        for manifest in self
            .metadata
            .list_manifests()?
            .into_iter()
            .filter(|m| m.commit_state == CommitState::Staging)
            .take(100)
        {
            if self
                .metadata
                .transfer(&manifest.object_id.to_string())?
                .is_some()
            {
                continue;
            }
            let Some(entry) = self
                .metadata
                .list_journal_entries()?
                .into_iter()
                .find(|j| j.object_id == manifest.object_id)
            else {
                continue;
            };
            if verify_staged_chunks(
                &self.staging_dir(entry.operation_id),
                &manifest,
                &self.encryption,
            )
            .is_err()
            {
                self.metadata
                    .update_manifest_state(manifest.object_id, CommitState::RecoveryRequired)?;
                continue;
            }
            let id = self.metadata.begin_transfer(
                manifest.object_id,
                &manifest.bucket,
                &manifest.key,
            )?;
            let local_chunks = manifest
                .chunks
                .iter()
                .filter(|chunk| !chunk.references_remote_payload())
                .collect::<Vec<_>>();
            self.metadata.reserve_staging(
                &id,
                local_chunks
                    .iter()
                    .map(|chunk| chunk.size.saturating_add(16))
                    .sum(),
                self.staging_budget,
            )?;
            self.metadata
                .queue_transfer(&id, entry.operation_id, local_chunks.len())?;
        }
        Ok(())
    }

    async fn cleanup_completed_staging(&self) -> Result<(), ObjectFormatError> {
        for job in self.metadata.transfers_for_local_cleanup(100)? {
            let mut paths = Vec::new();
            if let Some(operation) = job.operation_id.and_then(|s| Uuid::parse_str(&s).ok()) {
                paths.push(self.staging_dir(operation));
            }
            if let Ok(object_id) = Uuid::parse_str(&job.object_id) {
                paths.push(self.staging_dir(object_id));
                paths.push(
                    self.data_dir
                        .join(STAGING_ROOT)
                        .join(format!("upload-{object_id}")),
                );
            }
            paths.sort();
            paths.dedup();
            for path in paths {
                match async_fs::remove_dir_all(path).await {
                    Ok(()) => {}
                    Err(e) if e.kind() == io::ErrorKind::NotFound => {}
                    Err(e) => return Err(e.into()),
                }
            }
            self.metadata.with_connection(|c| {
                c.execute(
                    "UPDATE transfer_jobs SET state='cleaned',bytes=0 WHERE id=?1 AND state IN ('completed','cancelled','reception_failed','superseded')",
                    [job.id],
                )?;
                Ok(())
            })?;
        }
        Ok(())
    }

    pub(super) async fn process_cleanup_target(
        &self,
        target: &CleanupTarget,
    ) -> Result<(), ObjectFormatError> {
        if self.metadata.active_connection_id()?.as_deref() != Some(target.connection_id.as_str()) {
            self.metadata.quarantine_cleanup(
                target.id,
                &target.lease,
                "Cleanup belongs to a detached Telegram connection; recovery material retained",
            )?;
            return Ok(());
        }
        if Uuid::parse_str(&target.object_id)
            .ok()
            .is_some_and(|object_id| self.is_read_pinned(object_id))
        {
            self.metadata.fail_cleanup(
                target.id,
                &target.lease,
                5,
                "Cleanup deferred while a read is active",
            )?;
            return Ok(());
        }
        if target.kind == "evidence" {
            let path = self
                .data_dir
                .join(CLEANUP_EVIDENCE_ROOT)
                .join(format!("{}.json", target.object_id));
            if let Some(parent) = path.parent() {
                fs::create_dir_all(parent)?;
            }
            let manifest = Uuid::parse_str(&target.object_id)
                .ok()
                .and_then(|id| self.metadata.get_manifest(id).ok().flatten());
            let payload = serde_json::json!({
                "schema_version": 1,
                "kind": "telegram_s3_deletion_evidence",
                "object_id": target.object_id,
                "recorded_at": OffsetDateTime::now_utc(),
                "manifest": manifest,
            });
            let temporary = path.with_extension("json.tmp");
            let mut file = File::create(&temporary)?;
            file.write_all(&serde_json::to_vec_pretty(&payload)?)?;
            file.sync_all()?;
            drop(file);
            fs::rename(&temporary, &path)?;
            if let Some(parent) = path.parent() {
                sync_directory(parent)?;
            }
            let (attempt_token, _) = self.metadata.begin_cleanup_evidence_attempt(
                target.id,
                &target.lease,
                &target.object_id,
            )?;
            let location = match self
                .upload_local_file_to_telegram(&path, &attempt_token)
                .await
            {
                Ok(location) => location,
                Err(_) => {
                    self.metadata.quarantine_cleanup(
                        target.id,
                        &target.lease,
                        "Deletion-evidence acknowledgement is unknown; recovery material retained",
                    )?;
                    return Ok(());
                }
            };
            if !self
                .metadata
                .complete_cleanup(target.id, &target.lease, Some(&location))?
            {
                return Err(ObjectFormatError::InvalidPlan(
                    "cleanup evidence lease lost".into(),
                ));
            }
        } else {
            if !self.cleanup_location_is_referenced(target)? {
                let message_id = i32::try_from(target.message_id).map_err(|_| {
                    ObjectFormatError::InvalidPlan("cleanup message id out of range".into())
                })?;
                self.delete_telegram_messages(&[message_id]).await?;
            }
            if !self
                .metadata
                .complete_cleanup(target.id, &target.lease, None)?
            {
                return Err(ObjectFormatError::InvalidPlan("cleanup lease lost".into()));
            }
        }

        if self
            .metadata
            .cleanup_complete_for_object(&target.object_id)?
            && let Ok(object_id) = Uuid::parse_str(&target.object_id)
        {
            let manifest_path = self.manifest_path(object_id);
            match fs::remove_file(manifest_path) {
                Ok(()) => {}
                Err(e) if e.kind() == io::ErrorKind::NotFound => {}
                Err(e) => return Err(e.into()),
            }
            let chunk_dir = self.chunk_dir(object_id);
            match fs::remove_dir_all(chunk_dir) {
                Ok(()) => {}
                Err(e) if e.kind() == io::ErrorKind::NotFound => {}
                Err(e) => return Err(e.into()),
            }
        }
        Ok(())
    }

    async fn reconcile_cleanup_evidence(&self) -> Result<(), ObjectFormatError> {
        for attempt in self.metadata.cleanup_evidence_attempts(25)? {
            if self.metadata.active_connection_id()?.as_deref()
                != Some(attempt.connection_id.as_str())
            {
                continue;
            }
            let path = self
                .data_dir
                .join(CLEANUP_EVIDENCE_ROOT)
                .join(format!("{}.json", attempt.object_id));
            if !path.exists() {
                tracing::warn!(
                    target_id = attempt.id,
                    "cleanup evidence cannot be reconciled because its local copy is missing"
                );
                continue;
            }
            match self
                .reconcile_remote_file(&path, &attempt.token, attempt.started_at)
                .await
            {
                Ok(RemoteReconciliation::Match(location)) => {
                    self.metadata
                        .resolve_cleanup_evidence(attempt.id, &location)?;
                    tracing::info!(
                        target_id = attempt.id,
                        "cleanup evidence reconciliation matched the exact remote document"
                    );
                }
                Ok(RemoteReconciliation::Absent) => {
                    self.metadata.retry_cleanup_evidence_after_reconciliation(
                        attempt.id,
                        "Reconciled absent; a new uniquely-tokened evidence attempt is safe",
                    )?;
                    tracing::info!(
                        target_id = attempt.id,
                        "cleanup evidence reconciliation proved the remote document absent"
                    );
                }
                Err(error) => {
                    tracing::warn!(target_id = attempt.id, error = %error, "cleanup evidence reconciliation deferred");
                }
            }
        }
        Ok(())
    }

    fn cleanup_location_is_referenced(
        &self,
        target: &CleanupTarget,
    ) -> Result<bool, ObjectFormatError> {
        for manifest in self.metadata.list_manifests()? {
            if manifest.object_id.to_string() == target.object_id
                || manifest.commit_state == CommitState::Tombstoned
            {
                continue;
            }
            if (manifest.telegram.peer_id == target.peer_id
                && manifest.telegram.message_id == target.message_id)
                || manifest.chunks.iter().any(|chunk| {
                    chunk.telegram_peer_id == target.peer_id
                        && chunk.telegram_message_id == target.message_id
                })
            {
                return Ok(true);
            }
        }
        for session in self.metadata.list_multipart_sessions(None, None)? {
            for part in self.metadata.list_multipart_parts(session.upload_id)? {
                if (part.telegram.peer_id == target.peer_id
                    && part.telegram.message_id == target.message_id)
                    || part.manifest.as_ref().is_some_and(|manifest| {
                        manifest.chunks.iter().any(|chunk| {
                            chunk.telegram_peer_id == target.peer_id
                                && chunk.telegram_message_id == target.message_id
                        })
                    })
                {
                    return Ok(true);
                }
            }
        }
        Ok(false)
    }
}

pub(super) fn sync_directory(path: &Path) -> io::Result<()> {
    #[cfg(unix)]
    {
        File::open(path)?.sync_all()?;
    }
    #[cfg(not(unix))]
    let _ = path;
    Ok(())
}
