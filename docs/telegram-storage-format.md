# Telegram Storage Format

## Goals

- Rebuildable after local metadata loss
- Safe for object sizes larger than a single Telegram document limit
- Compatible with bounded-memory uploads and downloads
- Explicit about commit state

## Implementation Status

Phase 3 now implements the object-format service in this repository. Uploads
are chunked, checksummed, and staged through the journal before they become
visible, while startup reconciliation repairs complete staged uploads and
quarantines orphaned staging or chunk data. The RustFS-backed S3 server now
consumes the same manifest and chunk shape for bucket CRUD, object CRUD, and
range reads, so the layout is no longer just an internal placeholder.

## Design

Each object is represented by:

1. one canonical manifest document
2. one or more immutable chunk documents
3. a local index row and journal entry

## Default Chunk Size

The initial default is `1 MiB`, matching the conservative Telegram Drive
TDENC2 chunk size and staying far below Telegram's approximate `2,000,000,000`
byte file limit. The final implementation should keep the value configurable and
validate it against the Telegram client behavior in tests.

## Manifest Document

The manifest is a small JSON document. In phase 3, the object-format service
persists it through the local staging and commit paths under
`TELEGRAM_DATA_DIR`, while the future Telegram transport will publish the same
payload remotely. It must not depend on captions alone.

```json
{
  "schema_version": 1,
  "commit_state": "committed",
  "object_id": "uuid",
  "bucket": "photos",
  "key": "2026/08/image.jpg",
  "version_id": "optional-version-id",
  "content_length": 123,
  "content_type": "image/jpeg",
  "user_metadata": {},
  "tags": {},
  "created_at": "2026-08-29T00:00:00Z",
  "checksum": {
    "algorithm": "sha256",
    "whole_object": "hex"
  },
  "encryption": {
    "enabled": false,
    "format": "none",
    "key_id": null
  },
  "telegram": {
    "peer_id": "channel-or-chat-id",
    "message_id": 123,
    "document_id": "optional"
  },
  "chunks": [
    {
      "order": 0,
      "offset": 0,
      "size": 1048576,
      "checksum": "hex",
      "telegram_peer_id": "channel-or-chat-id",
      "telegram_message_id": 456,
      "telegram_document_id": "optional"
    }
  ]
}
```

## Chunk Documents

- Immutable after commit.
- Each chunk records order, offset, size, and checksum.
- Chunk documents are named with a stable object ID and chunk index.
- In phase 3, chunk payloads are stored as local durable files; phase 4 will
  map the same layout to Telegram documents.
- Chunk payloads must be independently verifiable.

## Local Index

The local SQLite database tracks the live object index and journal. In this
repo, that behavior is implemented by the versioned metadata store described in
`docs/metadata-store.md`.

The database tracks:

- bucket rows
- committed objects
- staging uploads
- tombstones
- orphaned chunks
- reconciliation state

The index is a fast path, not the only source of truth. Bucket rows follow the
same recovery rules as object manifests, so bucket visibility also depends on
the metadata store and reconciliation path.

## Commit State

Supported states:

- `staging`
- `committed`
- `tombstoned`
- `orphaned`
- `recovery_required`

## Recovery Rules

- A manifest without a local commit row is not visible until reconciliation.
- A staged upload without a manifest is aborted or resumed.
- A tombstone must survive long enough for background cleanup.
- Missing chunks make the object corrupt until repaired.
