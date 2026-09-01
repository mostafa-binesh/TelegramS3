# Disaster Recovery

## Local Metadata Lost

1. Stop the server.
2. Preserve the current Telegram session and data directories.
3. Run `telegram-s3 db status` to confirm the metadata path and schema.
4. Rebuild the local index with `telegram-s3 index rebuild`.
5. Verify object counts and checksum samples with `telegram-s3 index verify`.

## Telegram Manifest Lost

1. Mark the object corrupt or unrecoverable.
2. Attempt to recover from local backup copies if available.
3. If chunks remain but the manifest is missing, quarantine them as orphans.

## Interrupted Upload

1. On startup, inspect the operation journal.
2. The startup reconciliation pass now promotes complete staged uploads,
   recreates recovery markers for incomplete rows, and quarantines orphaned
   staging or chunk files.
3. Resume only if the upload state is safe to continue.
4. Otherwise roll back and clean up staging chunks or quarantined artifacts.

## Repair and Garbage Collection

1. Run `telegram-s3 repair --dry-run` first to see which staged, recovery-
   required, or orphaned rows will be reconciled.
2. Use `telegram-s3 repair` only after the dry-run shows the expected scope.
3. Run `telegram-s3 gc --dry-run` before cleanup to confirm only tombstoned
   objects older than the retention threshold are eligible.
4. Run `telegram-s3 gc` only when the dry-run output matches the intended
   cleanup scope.

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
   settings so the bind addresses and volume mounts remain consistent.
4. If only the image is lost, rebuild it and reattach the preserved volumes.
5. After restore, run `telegram-s3 doctor` or
   `docker compose exec telegram-s3 telegram-s3 doctor` to confirm the
   bootstrap path before returning traffic.
6. Use the authenticated `/_admin` dashboard to recheck storage overview,
   capacity, Telegram readiness, and bootstrap status before resuming writes.

## Telegram Session Loss

1. Keep the local metadata and data directories intact.
2. If the session file is missing or invalid, run `telegram-s3 auth login` to
   create a fresh Telegram session.
3. Confirm the new session with `telegram-s3 auth status`.
4. If Telegram rejects the session, use `telegram-s3 auth logout` and log in
   again with a fresh code.

## S3 Server Bootstrap Failure

1. Run `telegram-s3 doctor` to check the shared bootstrap path.
2. If doctor fails before the listener binds, inspect the Telegram session,
   proxy settings, bucket rows, and recovery markers first.
3. Fix the underlying object-format or Telegram transport issue before
   retrying `telegram-s3 server`.
4. A successful restart should preserve committed objects, keep staged work
   invisible, only make repaired data visible after reconciliation, and bind
   the loopback admin listener for `/healthz` and `/metrics`.
5. The same restart should also keep the authenticated `/_admin` operator
   frontend available for readiness and recovery checks.

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
