# Architecture

## Overview

Telegram S3 is designed around a four-layer flow:

```text
S3 client
  -> RustFS-compatible S3 server or fork
  -> Telegram storage adapter
  -> MTProto
  -> private Telegram chats/channels/messages
```

RustFS upstream already exposes a stable internal storage contract surface in
`crates/storage-api`, but the concrete backend in `ecstore` is still deeply
disk-oriented. Telegram Drive provides reusable ideas for Telegram transport,
session handling, retries, and encrypted envelopes.

## Major Components

- `s3-server` - S3 request handling, auth, routing, and HTTP responses.
- `storage-adapter` - bucket/object/multipart operations backed by Telegram.
- `local-index` - SQLite journal and lookup tables.
- `telegram-client` - MTProto session, login, proxy, retry, and transfer logic.
- `manifest-format` - canonical object state stored in Telegram.

## Request Lifecycle

### PUT

1. Validate bucket/object name and configuration.
2. Create an operation journal row.
3. Upload one or more chunks to Telegram in staging state.
4. Verify chunk size and checksum.
5. Upload the manifest.
6. Commit the local index atomically.
7. Make the object visible to readers.

### GET

1. Resolve the object through the local index.
2. Fetch the manifest.
3. Map the requested byte range to the needed chunks.
4. Stream chunks with bounded buffering and verification.
5. Reassemble plaintext or ciphertext bytes into the HTTP response.

### HEAD

1. Resolve the committed manifest.
2. Return object metadata from the local index and manifest.

### LIST

1. Use the local index for fast listing.
2. Reconcile with Telegram manifests when the index is stale or missing.

### DELETE

1. Record a tombstone or equivalent recoverable delete state.
2. Hide the object from reads and listings.
3. Queue physical cleanup of chunks and manifests.

## Multipart Uploads

- Multipart initiation creates a durable upload record.
- Each part is uploaded as a chunk or chunk group in staging state.
- Completion writes the final manifest, then commits the object atomically in the local index.
- Abort removes the upload record and schedules orphan cleanup.

## Telegram Layout

- One bucket maps to a stable namespace in Telegram.
- One object maps to a manifest plus one or more chunk documents.
- The manifest is the canonical recovery source.
- Chunks are immutable once committed.
- Deleted or replaced objects keep tombstone metadata until cleanup finishes.

## Manifest and Chunk Format

The format is documented in `docs/telegram-storage-format.md`. In short:

- manifest document stores bucket, key, object ID, version, metadata, checksum, and chunk references
- chunk documents store order, offset, size, and checksum
- local metadata stores fast lookup rows and journal state

## Metadata and Index

The local index must track:

- committed objects
- staging uploads
- tombstones
- orphaned chunks
- reconciliation status

The local database is an optimization and recovery journal, not the only source of truth.

## Crash Recovery

- Incomplete uploads remain invisible.
- Restart scans reconcile pending journals against Telegram.
- Manifest presence without local commit is repaired or rolled back.
- Local-only metadata loss can be rebuilt from manifests.

## Consistency

- Read-after-write is guaranteed only after commit.
- Overwrites preserve the previous committed object until the new manifest commits.
- Listings are only as fresh as the local journal and reconciliation lag allow.
- Delete visibility follows tombstone commit, not physical garbage collection.

## Cache Behavior

Cache candidates:

- manifest cache
- peer/channel resolution cache
- chunk lookup cache
- negative lookup cache when safe

All caches must be bounded and observable.

## Encryption Boundaries

- Telegram transport is not the same as object encryption.
- Optional object encryption must happen at the adapter boundary.
- Metadata protection and range-read support must be documented together.

## Error Mapping

- Telegram transport failures map to retryable or non-retryable storage errors.
- Flood waits should map to explicit backoff behavior, not silent retries.
- Partial writes must fail closed.

## Proxy Path

- Direct connection is the default.
- SOCKS5 and SOCKS5-with-auth are supported.
- HTTP/HTTPS proxy support can be bridged through a local SOCKS5 listener.
- Connectivity failure must be logged before fallback is attempted.

## Deployment Modes

- single-node local development
- production node with durable local metadata
- future clustered deployment with explicit replication boundaries

## Known Limitations

- Telegram is not a transactional object store.
- A 2 GiB Telegram file limit constrains object chunking and manifest design.
- Strong S3 versioning and object-lock semantics need compatibility layers.
- Presigned URL and server-side copy behavior must be explicitly tested.

