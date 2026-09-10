# Telegram Storage Format

## Goals

- Rebuildable after local metadata loss
- Safe for object sizes larger than a single Telegram document limit
- Compatible with bounded-memory uploads and downloads
- Explicit about commit state

## Implementation Status

Phase 3 now implements the object-format service in this repository. Uploads
are chunked, checksummed, staged through the journal, and then published as
Telegram documents/messages before they become visible, while startup
reconciliation repairs complete staged uploads and quarantines orphaned
staging or scratch data. Phase 5 extends that model with durable multipart
sessions and version-aware object copies, and phase 6 adds adapter-bound
envelope encryption plus recovery-aware repair and garbage collection around
the same layout, so the structure is now shared by single PUTs, multipart
completion, and the RustFS-backed S3 surface.

The authenticated operator frontend introduced in phase 8 reads the same
metadata and status surfaces for visibility, but it does not alter the storage
layout or relax the recovery rules described here. Its inline Telegram account
wizard saves bootstrap settings through the existing admin API; this is
operational UI only. If its hashed asset is unavailable, the console exposes a
retry state and does not change Telegram or local metadata.

Every send attempt is durable before the remote call. A restart that finds a
`recovery_required` job with no lease normalizes any stale `sending` attempt to
`unknown` and reconciles it by token plus exact encrypted bytes before deciding
whether a retry is safe.

## Design

Each object is represented by:

1. one canonical manifest document
2. one or more immutable chunk documents
3. a local index row and journal entry
4. optional multipart session and part rows while an upload is in progress

## Default Chunk Size

The initial default is `1 MiB`, matching the conservative Telegram Drive
TDENC2 chunk size and staying far below Telegram's approximate `2,000,000,000`
byte file limit. The configured chunk size is an upload-time policy, not a
format requirement: each committed manifest records the exact chunk references,
offsets, and sizes it uses. An operator may change `TELEGRAM_CHUNK_SIZE` for
later uploads after restarting the process; existing objects remain readable
and mixed chunk sizes are valid. Changing it does not rechunk or migrate
existing Telegram documents. A live database-backed setting could remove the
restart requirement, but it would still need to be captured per upload so one
transfer cannot change shape halfway through.

## Manifest Document

The manifest is a small JSON document. The committed manifest lives in the
local metadata store and is also published to Telegram as a document during
commit. `TELEGRAM_DATA_DIR` is only for transient staging, quarantine, and
recovery artifacts; it must not be the only copy of object bytes or metadata.
The manifest must not depend on captions alone.

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
  "expires_at": null,
  "checksum": {
    "algorithm": "sha256",
    "whole_object": "hex"
  },
  "encryption": {
    "enabled": true,
    "format": "chacha20poly1305-v1",
    "key_id": "key-fingerprint-or-hash"
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

`expires_at` is optional for backward-compatible manifests. When present, it
is an RFC3339 UTC timestamp. The local index and all S3/admin read and list
paths treat the object as missing at or after that instant, while the manifest
and Telegram chunks remain recoverable until normal tombstone retention and
garbage collection complete. The default local tombstone retention is 24 hours.
Share-link expiry is stored separately in local
metadata and can never extend beyond this manifest deadline.

## Encryption Envelope

- Chunk payloads are encrypted at rest with adapter-bound envelope encryption
  keyed from `TELEGRAM_S3_MASTER_KEY`.
- The manifest records whether encryption is enabled, which envelope format was
  used, and a stable key fingerprint for operator visibility and recovery
  checks.
- Range reads decrypt only the required chunk spans, so the server does not
  need to buffer whole objects in RAM.

## Chunk Documents

- Immutable after commit.
- Each chunk records order, offset, size, and checksum.
- Chunk documents are uploaded to Telegram and identified by peer/message
  metadata in the manifest.
- Each send also uses a durable, unique filename token. The token is forensic
  evidence only; the manifest's peer/message/document identifiers remain the
  authoritative object reference. If an acknowledgement is lost, recovery
  scans Telegram messages newer than the attempt boundary and accepts a match
  only when the token and complete encrypted bytes both match.
- Local disk keeps only temporary staging, quarantine, or mock transport
  artifacts.
- Chunk payloads must be independently verifiable.

## Local Index

The local SQLite database tracks the live object index and journal. In this
repo, that behavior is implemented by the versioned metadata store described in
`docs/metadata-store.md`.

The database tracks:

- bucket rows (created through the S3 API or the authenticated `/_admin` UI)
- committed objects
- staging uploads
- tombstones
- orphaned chunks
- reconciliation state
- app settings, including validated Telegram bootstrap settings used to resolve
  the storage session and chat before transport startup

The index is a fast path, not the only source of truth. Bucket rows follow the
same recovery rules as object manifests, so bucket visibility also depends on
the metadata store and reconciliation path. Both legacy `ListObjects` and
`ListObjectsV2` read from that same ordered local index rather than deriving
separate views of the bucket.

The admin bucket browser preserves bucket names exactly, including Unicode
characters. Its delete action only requests deletion of an empty bucket; it does
not alter manifest, tombstone, or recovery semantics.

Removing a Telegram connection tombstones every visible local object and marks
all buckets deleted in one metadata transaction, so the active index and its
statistics disappear immediately. The removal job records the active
connection generation. Transfer jobs, multipart sessions, manifests, and
buckets snapshot that generation when created; removal only cancels and purges
operational records carrying the removed generation. The optional remote-delete choice adds every manifest
location to the existing evidence-first cleanup outbox, with the same
generation copied onto each target. When remote deletion is selected, local
visibility is removed immediately but the owning generation/transport remains
reserved until the evidence-first queue completes. Cleanup workers refuse to
use a different generation for old targets and quarantine them for recovery
instead. If remote deletion is not selected, Telegram payloads remain by design
and cannot be treated as a managed backup after the connection is removed.

## Commit State

Supported states:

- `staging`
- `committed`
- `tombstoned`
- `orphaned`
- `recovery_required`

Multipart sessions use their own local states:

- `initiated`
- `uploading`
- `completing`
- `completed`
- `aborted`
- `recovery_required`

Browser resumable receptions use the same encrypted staging directory and
`transfer_jobs` receiving state as one-shot admin uploads. The reception ID is
the transfer/object UUID, and each PATCH must supply the server's current byte
offset. A final chunk is required before the manifest can be written and the
job can enter the durable queue. Reception progress is held in process memory;
staged bytes remain encrypted at rest, but a process restart ends the browser
session and reconciliation handles the stale receiving job as recovery work.

## Recovery Rules

- A manifest without a local commit row is not visible until reconciliation.
- A staged upload without a manifest is aborted or resumed.
- A tombstone must survive until the evidence-first background cleanup worker
  has removed its Telegram messages, or until an operator reviews a
  generation-mismatch/recovery-required target; cleanup is due immediately and
  remains retryable if Telegram is unavailable.
- Missing chunks make the object corrupt until repaired.
- Recovery and transfer attention records are local operational state. Removing
  their owning connection hides them immediately and deletes them after cleanup
  dependencies are satisfied; it does not claim remote Telegram deletion unless
  the remote-delete option was selected.
- An ambiguous send first enters `unknown`. The background worker can resolve
  it to `checkpointed` after an exact remote match, or to `retryable` after a
  complete history scan finds no matching document. An incomplete scan,
  unavailable Telegram session, missing staging file, or byte mismatch stays
  `recovery_required` for operator review.
- Multipart parts remain hidden until completion publishes the final manifest.
- Version IDs are derived from the stored manifest identity, so copy and delete
  marker flows can remain explicit across restarts.
