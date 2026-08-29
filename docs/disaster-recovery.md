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

## Telegram Session Loss

1. Keep the local metadata and data directories intact.
2. If the session file is missing or invalid, run `telegram-s3 auth login` to
   create a fresh Telegram session.
3. Confirm the new session with `telegram-s3 auth status`.
4. If Telegram rejects the session, use `telegram-s3 auth logout` and log in
   again with a fresh code.

## Orphan Cleanup

- Run garbage collection in dry-run mode first.
- Never delete uncertain data without operator confirmation.
- Keep an audit summary of the cleanup scope.

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
