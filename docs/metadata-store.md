# Metadata Store

## Purpose

Telegram S3 uses a local SQLite database as the authoritative fast path for
object visibility, recovery, and index rebuilding. Telegram remains the remote
durable envelope for manifests and chunks, but the service never relies on
Telegram alone to answer read/write consistency questions.

## Schema Version

- Current schema version: `1`
- Version contract: migrations are applied on startup and are also available
  through the `telegram-s3 db migrate` command.
- Startup behavior: the store opens the configured SQLite file, creates the
  schema if needed, rebuilds the active index, and records recovery markers for
  staged operations. The object-format bootstrap layer now uses the same store
  to reconcile staged uploads, recovery-required objects, and quarantined
  orphans before `doctor` or `server` report success.

## Tables

- `schema_version`
  - single-row version table
  - stores the current schema version and the last applied timestamp
- `object_manifests`
  - one row per object manifest revision
  - stores the full manifest JSON and commit state
- `operation_journal`
  - one row per staged operation
  - stores the operation kind, current state, and serialized recovery details
- `active_objects`
  - fast-path bucket/key pointer to the currently visible committed object
- `recovery_markers`
  - durable markers for staged or otherwise incomplete operations

The phase 3 object-format service also persists chunk and manifest documents
under the configured data directory, using the metadata store as the
authoritative fast path and recovery journal.

## Write Path

1. Stage the manifest and journal entry.
2. Write or refresh the recovery marker.
3. Commit the manifest.
4. Update the active pointer for the bucket/key pair.
5. Remove the staging recovery marker.

This preserves overwrite semantics by keeping the previous committed object
visible until the new manifest commits.

## Recovery and Rebuild

- `telegram-s3 doctor` opens the database and prints the current metadata
  status. It also bootstraps the object-format layer and fails fast if staged
  uploads or recovery-required objects remain unresolved.
- `telegram-s3 db status` reports the stored schema version and row counts.
- `telegram-s3 db migrate` applies outstanding migrations.
- `telegram-s3 index rebuild` repopulates the active index from committed
  manifests and refreshes staging markers.
- `telegram-s3 index verify` compares the active index against committed
  manifest rows and fails if mismatches remain.

## Notes

- The local SQLite path defaults to `data/metadata.sqlite` when the
  corresponding environment variable is not provided.
- Staged uploads are not visible to readers until commit.
- Tombstones stay in the historical manifest table so cleanup can be audited
  and rebuilt later.
