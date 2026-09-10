use super::*;
use crate::durable::{CleanupTarget, TransferJob, TransferWriteConditionals};
use std::sync::atomic::Ordering;

impl ObjectFormatService {
    /// Accept the complete request on durable local storage; remote work is independent.
    pub async fn enqueue_stream(
        &self,
        bucket: &str,
        key: &str,
        content_type: &str,
        body: Option<StreamingBlob>,
        conditionals: Option<TransferWriteConditionals>,
    ) -> Result<TransferJob, ObjectFormatError> {
        self.ensure_connection_not_removing()?;
        self.enqueue_with_part(bucket, key, content_type, body, None, conditionals)
            .await
    }

    pub(super) async fn enqueue_with_part(
        &self,
        bucket: &str,
        key: &str,
        content_type: &str,
        body: Option<StreamingBlob>,
        part: Option<(Uuid, u32, Option<String>)>,
        conditionals: Option<TransferWriteConditionals>,
    ) -> Result<TransferJob, ObjectFormatError> {
        self.ensure_connection_not_removing()?;
        let object_id = Uuid::new_v4();
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
            let mut pending = Vec::with_capacity(self.chunk_size as usize);
            let mut chunks = Vec::new();
            let mut hasher = Sha256::new();
            let mut offset = 0;
            while let Some(frame) = body.next().await {
                let bytes = frame.map_err(|_| {
                    ObjectFormatError::InvalidPlan(
                        "request body interrupted; resend source file".into(),
                    )
                })?;
                let mut remaining = bytes.as_ref();
                while !remaining.is_empty() {
                    let take = remaining
                        .len()
                        .min(self.chunk_size as usize - pending.len());
                    pending.extend_from_slice(&remaining[..take]);
                    remaining = &remaining[take..];
                    if pending.len() == self.chunk_size as usize {
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
                                ObjectFormatError::Telegram(_) => (
                                    Some(
                                        crate::telegram::retry::parse_flood_wait_seconds(
                                            error.to_string(),
                                        )
                                        .unwrap_or(2u64.saturating_pow(job.attempts.min(8)))
                                        .max(1),
                                    ),
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
        let mut recovery_shutdown = shutdown_rx.clone();
        let recovery_handle = tokio::spawn(async move {
            let _ = service.refresh_recovery_snapshot().await;
            loop {
                if *recovery_shutdown.borrow() {
                    break;
                }
                tokio::select! {
                    _ = tokio::time::sleep(std::time::Duration::from_secs(60)) => {
                        let _ = service.refresh_recovery_snapshot().await;
                    }
                    _ = recovery_shutdown.changed() => {}
                }
            }
        });
        let service = self.clone();
        let mut cleanup_shutdown = shutdown_rx;
        let cleanup_handle = tokio::spawn(async move {
            loop {
                if *cleanup_shutdown.borrow() {
                    break;
                }
                let _ = service.finalize_connection_removal().await;
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
                    Ok(None) => tokio::select! {
                        _ = tokio::time::sleep(std::time::Duration::from_secs(1)) => {},
                        _ = cleanup_shutdown.changed() => {},
                    },
                    Err(_) => tokio::select! {
                        _ = tokio::time::sleep(std::time::Duration::from_secs(2)) => {},
                        _ = cleanup_shutdown.changed() => {},
                    },
                }
            }
        });
        if let Ok(mut handles) = self.worker_runtime.handles.lock() {
            handles.push(transfer_handle);
            handles.push(recovery_handle);
            handles.push(cleanup_handle);
        }
    }

    async fn finalize_connection_removal(&self) -> Result<(), ObjectFormatError> {
        let Some(job) = self.metadata.connection_removal_job()? else {
            return Ok(());
        };
        if job.state == "remote_cleanup_pending" {
            if !self.metadata.connection_removal_cleanup_pending(&job.id)? {
                self.detach_connection_if_owned(&job).await?;
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
                "cancelled" | "recovery_required" => {
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
                            let location = self
                                .upload_local_file_to_telegram(&path, &chunk_file_name(chunk.order))
                                .await?;
                            self.metadata.finish_send_attempt(
                                &job.id,
                                lease,
                                chunk.order,
                                &attempt,
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
            .buffer_unordered(2);
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
                let location = self
                    .upload_local_file_to_telegram(&path, MANIFEST_FILE_NAME)
                    .await?;
                self.metadata
                    .finish_send_attempt(&job.id, lease, u32::MAX, &attempt, &location)?;
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
            self.metadata.reserve_staging(
                &id,
                manifest.content_length + manifest.chunks.len() as u64 * 16,
                self.staging_budget,
            )?;
            self.metadata
                .queue_transfer(&id, entry.operation_id, manifest.chunks.len())?;
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
            let location = match self
                .upload_local_file_to_telegram(&path, "deletion-evidence.json")
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
