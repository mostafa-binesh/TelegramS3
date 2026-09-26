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
  "schema_version": 2,
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
      "telegram_document_id": "optional",
      "source_object_id": "optional-source-uuid",
      "source_chunk_order": 0
    }
  ]
}
```

Schema v1 manifests remain readable. Schema v2 adds the optional paired
`source_object_id` and `source_chunk_order` fields. They are absent for payloads
encrypted directly for the current manifest. Multipart composition sets both
fields so the reader derives the key, nonce, and authenticated-data identity
from the part that originally produced the immutable Telegram document. The
pair also marks the document as an existing remote reference: publication must
not look for a local staged chunk or upload the payload again.

`expires_at` is optional for backward-compatible manifests. When present, it
is an RFC3339 UTC timestamp. The local index and all S3/admin read and list
paths treat the object as missing at or after that instant, while the manifest
and Telegram chunks remain recoverable until normal tombstone retention and
garbage collection complete. The default local tombstone retention is 24 hours.
Share-link expiry is stored separately in local metadata and can never extend
beyond this manifest deadline. Each share link stores a hash of its opaque
bearer token for lookup, plus ciphertext encrypted under a key derived from
`TELEGRAM_S3_MASTER_KEY` so the authenticated admin panel can list and manage
the link after creation. Descriptions are operator metadata and are limited to
240 characters. Schema v12 adds the ciphertext and description columns.
Legacy hash-only rows remain usable by their existing public URL and can be
revoked, but the admin panel cannot reconstruct their URL.

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

## Multipart Manifest Composition

Each completed part has a private manifest and one or more already-published
Telegram chunk documents. Completion validates the requested part order, ETags,
sizes, checksums, manifest shape, Telegram locations, and encryption metadata.
It then builds one contiguous final chunk map by assigning final orders and
offsets while retaining each source message/document location and payload
identity. Only the final canonical manifest is sent to Telegram.

Ordinary PUT manifests use `sha256`, whose value is the digest of the complete
plaintext object. A composed multipart manifest uses `sha256-parts-v1`: SHA-256
over a domain separator followed by each ordered part number, byte size,
checksum algorithm, and already-verified part checksum, with variable strings
length-prefixed. This is a deterministic composite object identity, not a claim
to be the raw-byte SHA-256 of the concatenated payload. Every reused chunk keeps
its plaintext SHA-256 and is independently decrypted and verified during full
or range reads.

The final SQLite commit changes object visibility and the multipart session to
`completed` atomically. Cleanup queues the private part-manifest messages but
filters out every chunk location referenced by the final manifest. Those chunks
become owned by the committed object and are eligible for physical cleanup only
after that object is tombstoned under the normal evidence-first rules.

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
characters. Directory markers represent empty folders in the browser view: an
admin folder delete is rejected while active child objects or pending child
transfers exist. Object and empty-folder deletes commit a local tombstone before
reporting success; physical Telegram cleanup remains asynchronous and
evidence-first.

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

Active S3 transfer jobs are also exposed to the authenticated bucket browser.
The UI may show a key as receiving, uploading, waiting for Telegram,
finalizing, or needing attention before its manifest is committed, using the
job's part/chunk counters and derived durable state. `N/N` with `completing`
means payload publication is done and the final manifest is being published.
`retry_wait` means Telegram explicitly requested pacing and the worker will
resume automatically. This is operational visibility only and does not add the
key to the committed object index. Partial or recovery-required data therefore
remains unavailable to download until completion or explicit repair.

The authenticated browser keeps a foreground folder listing authoritative while
that request is pending. Its silent multipart/activity poll waits rather than
superseding the listing request, so a slow Telegram-backed response cannot leave
the UI displaying an indefinite loading state. This affects presentation only;
manifest visibility and committed-object rules remain unchanged.

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
- An explicit Telegram `FLOOD_WAIT` is a known pre-publication rejection, not
  an ambiguous acknowledgement. Its send attempt enters `retryable`, its job
  enters `retry_wait`, and the worker preserves Telegram's requested pause
  before retrying. Older persisted flood-wait rows are upgraded only after any
  unrelated `sending` or `unknown` attempt has completed normal reconciliation.
- Multipart parts remain hidden until completion publishes the final manifest.
- A repeated in-flight `UploadPart` attaches to the earliest durable job for
  that upload ID and part number instead of publishing a second payload. A
  duplicate is superseded only when it has no `checkpointed`, `sending`, or
  `unknown` Telegram attempt; anything that may own a remote document remains
  recoverable for normal reconciliation or operator review.
- Multipart completion reuses only chunk references whose source identity and
  Telegram location are explicit in schema v2; missing provenance or an
  unpublished source chunk fails completion without exposing an object.
- An abandoned pre-composition completion may be superseded only when no
  `sending`/`unknown` Telegram attempt remains. Ambiguous acknowledgements keep
  the session recovery-required until token-and-exact-byte reconciliation.
- An active multipart key may be visible in the admin browser as an uploading
  status row with progress, but it remains hidden from committed S3 listings and
  download actions until completion publishes the final manifest.
- Version IDs are derived from the stored manifest identity, so copy and delete
  marker flows can remain explicit across restarts.
