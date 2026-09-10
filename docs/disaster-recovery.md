# Disaster Recovery

## Local Metadata Lost

1. Stop the server.
2. Preserve the current Telegram session and data directories.
3. Restore `metadata.sqlite` from backup if you have one, then run `telegram-s3 db status` to confirm the metadata path and schema.
4. Rebuild the local index with `telegram-s3 index rebuild`.
5. Verify object counts and checksum samples with `telegram-s3 index verify`.

> The same `metadata.sqlite` now also stores operator accounts, Telegram
> bootstrap settings, connection-generation state, and session tombstones
> (schema v8). A backup/restore of
> that file restores both object
> state and who can sign in. If accounts are lost, re-provision the first
> operator with `telegram-s3 users create <username> --password <pw>` (the first
> account becomes the superadmin). Password hashes are argon2id + per-account
> salt and are not recoverable from the file alone. The object rows also hold
> the Telegram peer/message/document references, so losing the metadata store
> means losing the index that points at the Telegram payloads.

## Telegram Manifest Lost

1. Mark the object corrupt or unrecoverable.
2. Attempt to recover from a surviving metadata backup or from the source data
   that produced the object.
3. If the committed Telegram document is missing, treat the object as corrupt
   until it is repaired or re-uploaded.

## Interrupted Upload

1. On startup, inspect the operation journal.
2. The startup reconciliation pass now promotes complete staged uploads,
   recreates recovery markers for incomplete rows, and quarantines orphaned
   staging artifacts.
3. Resume only if the upload state is safe to continue.
4. Otherwise roll back and clean up staging or quarantined artifacts.

### Interrupted Admin Browser Reception

The admin resumable upload API only resumes browser-to-server reception. The
browser keeps the reception ID and re-reads the authoritative offset after a
failed PATCH, but the server holds the active reception map in memory. Resume
only while the receiving lease is alive; after a server restart or expired
lease, re-select the source file and start a new reception. Completed
receptions are independent durable transfer jobs and continue through the
normal worker/reconciliation path.

## Repair and Garbage Collection

1. Run `telegram-s3 repair --dry-run` first to see which staged, recovery-
   required, or orphaned rows will be reconciled.
2. Use `telegram-s3 repair` only after the dry-run shows the expected scope.
3. The cleanup worker drains tombstone outbox entries automatically. Run
   `telegram-s3 gc --dry-run` when reviewing older tombstones or a manual
   cleanup scope; Telegram message removal remains evidence-first and retryable.
4. Run `telegram-s3 gc` only when the dry-run output matches the intended
   cleanup scope.

## Removing the Current Telegram Connection

The admin Connection tab requires confirmation before removal. The transaction
immediately hides buckets, active objects, recovery markers, and related
statistics, while the durable `connection_removal_jobs` record keeps the scope
restart-safe. Recovery-required manifests, cancelled transfers, multipart
sessions, and local attention rows owned by that generation disappear from the
panel immediately; their operational records are purged after safe
finalization. Selecting **Also delete all uploaded Telegram files** enqueues
manifest and chunk messages for the evidence-first cleanup worker. The owning
Telegram generation and transport remain reserved until that queue completes;
this prevents the worker from deleting through a replacement account. Each
target stores its connection generation; if that generation is no longer
owned by the removal job, the worker quarantines the target instead of guessing
which account may authorize it.
Unknown Telegram acknowledgements remain `recovery_required` and keep evidence
material until the worker can inspect them. The worker records a unique send
token before upload, searches Telegram history back to the attempt timestamp,
and repairs a checkpoint only for an exact encrypted-byte match. If no match is
found after a complete scan, it schedules the chunk for retry; connectivity,
scan-limit, missing-staging, and byte-mismatch cases remain recovery-required.
On restart, a job already in `recovery_required` with no lease also has stale
`sending` rows normalized to `unknown` before reconciliation. That
normalization is committed even when no new transfer is available to claim, so
the recovery queue cannot roll back and leave Retry permanently blocked.
If the checkbox is not selected, local data is still removed from the active
installation but remote Telegram files are intentionally left in place.
Do not describe those retained files as deleted or recoverable through the
removed connection.

After cleanup completes, reconnect and verify that the overview reports `connected`, not merely
`authorized` or `reused`. A `needs_reauth` state with
`AUTH_KEY_UNREGISTERED` means the Telegram session is no longer valid and must
be logged in again; it is not a storage-peer lookup problem that a new delete
request will fix.

## Interrupted Multipart Upload

1. Check `telegram-s3 auth status` first if the session was refreshed around
   the same time as the failure.
2. Multipart sessions are durable in local metadata, so restart reuse should
   preserve the upload ID and uploaded parts.
3. If a multipart session is marked `recovery_required`, abort or repair it
   before trying to complete the upload.
4. If the session files are gone but the local session row remains, clean up
   the multipart metadata and retry the upload from a fresh initiate call.

## Docker Deployment Loss

1. Stop the `telegram-s3` container before restoring state.
2. Preserve the named metadata, data, and session volumes together if the
   container is still healthy enough to inspect.
3. Recreate the container from the same `docker-compose.yml` and `.env`
   settings so the bind addresses and volume mounts remain consistent. If the
   Telegram bootstrap settings were already saved in the admin panel, they come
   back with the `metadata.sqlite` volume.
4. If only the image is lost, rebuild it and reattach the preserved volumes.
5. After restore, run `telegram-s3 doctor` or
   `docker compose exec telegram-s3 telegram-s3 doctor` to confirm the
   bootstrap path before returning traffic.
6. Use the authenticated `/_admin` dashboard to recheck storage overview,
   capacity, Telegram readiness, bucket visibility (including buckets created
   from the UI), and bootstrap status before resuming writes.
7. If a console view remains on a loading skeleton, inspect the response for
   its hashed file under `/_admin/assets/`. Rebuild/redeploy the image with the
   complete UI `assets/` directory, then use the view's Retry action; this is a
   UI-asset issue and does not alter the persisted Telegram settings.

The Telegram setup dialog owns an operator-scoped flow id. Closing the dialog
cancels that flow, and reopening it requests a fresh code. An invalid code keeps
the current attempt retryable; an expired code requires starting a new attempt.
Stale browser requests cannot cancel or advance a replacement flow.

## Telegram Session Loss

1. Keep the local metadata and data directories intact.
2. If the session file is missing or invalid, run `telegram-s3 auth login` to
   create a fresh Telegram session.
3. Confirm the new session with `telegram-s3 auth status` and the `/_admin`
   overview card, which should move to connected once the storage chat lookup
   succeeds.
4. If Telegram rejects the session, use `telegram-s3 auth logout` and log in
   again with a fresh code.

## S3 Server Bootstrap Failure

1. Run `telegram-s3 doctor` to check the shared bootstrap path.
2. If doctor fails before the listener binds, inspect the Telegram session,
   proxy settings, bucket rows, and recovery markers first.
3. If admin-side Telegram settings were changed shortly before the failure,
   verify that the persisted API ID and storage chat ID are numeric before
   retrying transport startup or login.
4. Fix the underlying object-format or Telegram transport issue before
   retrying `telegram-s3 server`.
5. A successful restart should preserve committed objects, keep staged work
   invisible, only make repaired data visible after reconciliation, and bind
   the loopback admin listener for `/healthz` and `/metrics`.
6. The same restart should also keep the authenticated `/_admin` operator
   frontend available for readiness and recovery checks.
7. Verify bucket and object visibility through the client class you depend on
   (`ListObjects` legacy callers as well as `ListObjectsV2` callers) before
   returning the endpoint to backup tooling.

## Orphan Cleanup

- Run garbage collection in dry-run mode first.
- Never delete uncertain data without operator confirmation.
- Keep an audit summary of the cleanup scope.
- The live `gc` command only removes tombstoned data that is safely past the
  retention threshold; uncertain data stays quarantined.

## Recovery Commands

- `telegram-s3 doctor`
- `telegram-s3 auth status`
- `telegram-s3 auth logout`
- `telegram-s3 auth login`
- `telegram-s3 db status`
- `telegram-s3 db migrate`
- `telegram-s3 index rebuild`
- `telegram-s3 index verify`
- `telegram-s3 repair`
- `telegram-s3 gc --dry-run`
