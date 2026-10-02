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
The Accounts/Connections view likewise does not change the storage layout: it
loads account state on entry and uses an explicit operator refresh instead of
periodically polling account metadata.

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

Schema v21 additionally stores an account registry, durable replication jobs,
selected-object replication scopes, replica/access locations, and re-chunk
locks/jobs. Account rows include `download_enabled`, an additive read-selection
policy that defaults to enabled for existing data. The `object_keys_json` migration is additive: an empty array keeps
existing whole-bucket replication behavior, while a populated array limits a
job to the selected keys. These rows are additive and do not rewrite existing
object ownership. Re-chunk jobs also persist their source account, the selected
replica policy, and a snapshot of target account/mode pairs before the old
manifest is replaced. `ChunkRef.replicas` is optional so
schema-v2 manifests remain readable; each location identifies its account,
mode (`replica` or `access`), peer, message, and document. The primary chunk
location remains authoritative for legacy manifests.
New replica locations also persist `chunk_size`, the exact canonical chunk size
whose encrypted bytes they represent. A non-zero replica value that differs
from its canonical `ChunkRef.size` is exposed by the admin API as a replica
layout mismatch and rendered as a yellow account badge. Older replica rows
without this field remain readable and are treated as unknown rather than
incorrect.
The verifier also stores a durable integrity event for each sampled account and
chunk. Events retain the account label, failure details, and repair state across
restarts without changing the manifest's committed state when only an alternate
replica is affected.

Verifier scans emit start/end or failure log records and maintain process-local
run, failure, timestamp, and duration metrics for the operator Overview. These
metrics describe the scan work; integrity findings remain represented by the
durable recovery-event and manifest state described above.

Cleanup targets use evidence-first ordering. Schema v21 adds indexed claim and
object-dependency paths so an idle worker does not repeatedly scan the entire
outbox. The worker backs off to the next actionable window, capped at one
minute so newly-created due work remains responsive. Ambiguous evidence uploads
persist a unique attempt token and start time; exact-token and exact-byte
reconciliation must prove absence before a replacement attempt is allowed.
Rows created before attempt-token persistence remain quarantined rather than
being guessed through.

Replication copies encrypted chunk bytes, preserving the object checksum and
encryption identity. Access-only replication records the source location and
requires the target Telegram session to have access to that group/chat.
Re-chunking stages a replacement object with a new chunk policy, then commits
it through the normal transfer journal. A lock prevents mixed old/new reads.
The storage operation is object-scoped. The admin bucket selection may expand
selected buckets into one durable job per committed object before invoking the
worker. Replica locations belong to the old manifest;
they are not silently treated as valid locations for the replacement chunk
layout and are handled by the old manifest's evidence-first cleanup. Re-run
replication after re-chunking when alternate physical copies are required.
If the re-chunk policy is enabled, those follow-up replications are queued
after the replacement manifest commits. If it is disabled, the replacement
manifest intentionally has no old replica paths and round-robin resumes only
after a new replication job completes.
The shared reader filters primary and replica locations by the account download
policy before its deterministic round-robin selection. If every location is
disabled, the read fails closed with an unavailable-source error; no account
policy can redirect cleanup through another credential.
When a selected eligible location fails, the reader makes the configured
number of complete retries from `telegram_download_failover_retries` (default
`1`, range `0–8`) before moving to the next eligible location. This policy is
stored in `app_settings`, survives restart, and is independent of the normal
Telegram transport retry count. A chunk is still fetched, decrypted, and
verified as one unit; it is not split between accounts.

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

## Sampled Recovery Verification

The background verifier checks committed objects without downloading every
chunk on every pass. For each healthy committed manifest it selects up to the
configured number of distinct chunk indexes uniformly at random, downloads and
decrypts the primary document and every recorded replica for each sampled
chunk, and verifies their plaintext checksums. The
interval, sample count, and enabled state are stored in `app_settings` and can
be changed from the authenticated Storage policy page. Disabling the verifier
stops its automatic worker scans while preserving the other policy values and
existing recovery findings. Environment variables only seed missing database
settings.
The same page can queue a manual verifier run; it uses the worker's normal
sampling and recovery path, and the configured interval begins again when that
run completes. No manifest or migration data is needed for the trigger.

The Storage policy page can also wake the cleanup worker with **Run eligible
cleanup now**. The worker still uses each target's persisted `due_at`, records
deletion evidence before removing Telegram messages, preserves shared-message
references and active-read pins, and leaves `recovery_required` targets for
operator recovery. This wake action requires no manifest or schema migration.

A confirmed missing Telegram message, decryption failure, or checksum mismatch
on a physical replica is logged as a per-account integrity event. If another
location verifies successfully, the server uploads the exact verified encrypted
bytes to the damaged account and updates that replica location. A failed
physical primary is repaired from the same alternate source when possible. An
access-only location is not re-uploaded because it is only a pointer into a
shared chat. A transient Telegram/network failure is retained as retryable and
rechecked later. Only an unrecoverable primary failure transitions the manifest
to `recovery_required` and removes it from `active_objects`; a healthy alternate
replica keeps the object downloadable while its repair event remains visible to
the operator.

## Manifest Document

The manifest is a small JSON document. The committed manifest lives in the
local metadata store and is also published to Telegram as a document during
commit. The Telegram document's filename is the opaque, durable send-attempt
token rather than `manifest.json` and is intentionally extensionless. The
bytes are still the JSON manifest; the token lets ambiguous sends be
reconciled by exact filename/token and exact bytes instead of captions. The
same naming rule applies to cleanup-evidence documents, whose contents are
JSON evidence records. `TELEGRAM_DATA_DIR` is only for transient staging,
quarantine, and recovery artifacts; it must not be the only copy of object
bytes or metadata. The manifest must not depend on captions alone.

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
garbage collection complete. The default local tombstone retention is 12 hours;
the value is persisted in `app_settings` and is configurable from Storage
policy for future delayed cleanup targets.
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

## Read Streaming, Retry, and Resume

S3 and public-share reads fetch the referenced Telegram document for one
manifest chunk at a time, decrypt and checksum-verify that complete chunk, and
only then emit its requested byte span. A transient Telegram RPC or transport
I/O failure discards the partial fetch and retries the same message under the
configured bounded retry/flood-wait policy. The server never emits a partial or
unverified chunk before retrying.

The shared reader supports bounded parallel prefetching. The database-backed
`telegram_download_prefetch_chunks` policy allows `0–4` extra chunks in flight
and defaults to `1`; `0` keeps the reader serial. Prefetched chunks remain
ordered behind the current output, and every prefetched chunk is fully fetched,
decrypted, and checksum-verified before emission. This reduces normal
chunk-boundary idle gaps without buffering a whole object; the maximum active
window is `prefetch + 1` chunks. The actual window is adaptive: it ramps up
after clean reads and backs off after retries or measured throughput drops. A
per-account limiter keeps one active Telegram payload read per account, so
different enabled replicas can still provide parallel capacity. Dropping the
client stream drops outstanding prefetch work as well.

The database-backed `telegram_download_prefetch_mode` setting (schema v23)
selects `adaptive` parallel scheduling or `sequential` nearest-chunk scheduling.
Sequential mode reads the first span alone, then fetches later spans strictly
in manifest order, one at a time, with at most the configured prefetch count
waiting ahead of the client. It changes runtime scheduling only; manifests,
chunk references, and checksums are unchanged.

The first requested span is intentionally fetched on its own before the
speculative window is opened. This first-chunk priority improves time to first
byte while preserving manifest order and the same bounded concurrency limit.
The authenticated Overview exposes a bounded, process-local testing snapshot
for recent reads: surface, chunk and payload counts, first-chunk latency,
Telegram and retry-wait time, decrypt time, checksum-verification time, and
total duration, plus the final/max adaptive window and per-account Telegram
chunk, byte, retry, and duration totals. The snapshot retains only the latest 20 samples and is not
part of the manifest, metadata journal, recovery state, or traffic totals. Active
reads are held in process memory and report object, mode, current chunk,
client-delivered bytes, Telegram-read bytes, retries, and server/client chunk
counts without adding per-chunk SQLite writes.

The Overview keeps manifest-derived object counts and unique committed Telegram
payload size in a five-second cache. Its five-second background refresh reads
the separate `/overview/live` surface for transfer, traffic, stage, and
connection telemetry, so routine dashboard polling does not reparse every
manifest. Stage samples include client-emitted and Telegram-read byte totals,
and a response that delivers its complete requested range is completed even if
the HTTP consumer drops the stream before polling the terminal end marker. A
manual/full Overview refresh can still update the cached storage
snapshot.

Public share and authenticated admin responses advertise `Accept-Ranges: bytes`
and support a single `Range` request with `Content-Range`. During a streamed
read, transient Telegram/proxy failures keep the response open while the same
chunk is retried for up to 120 seconds. If that recovery window is exhausted,
the response ends with the already-sent bytes intact; a capable client can
request the remaining range and resume. Missing Telegram messages, decryption
failures, and checksum mismatches are not treated as transient network errors
and remain recovery signals.
Public media manifests retain their stored `audio/*` or `video/*` content type
and receive inline disposition at the HTTP boundary, so players can probe the
URL without changing the encrypted chunk or manifest format. Other public
objects retain attachment disposition.

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

The metadata store uses a bounded pool of eight SQLite handles. File-backed
handles use WAL, `FULL` synchronous durability, foreign-key enforcement, and a
30-second busy timeout. A low-level handle failure is isolated and replaced;
this improves process resilience and concurrent read throughput but does not
repair a corrupt metadata file. The pool does not change the manifest format or
require a schema migration.

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

The server does not verify every committed Telegram chunk before binding its
listeners. When the persisted startup-scan policy is enabled, the first full
recovery snapshot runs in the background worker after startup and updates the
local recovery view; when it is disabled, the first remote scan waits for the
configured verifier interval. Either choice keeps health and operator access
available while a slow or unavailable Telegram read is still being checked.

The authenticated object browser performs moves on the server. A file move
creates a durable destination manifest with the original encrypted chunk
references, publishes only that small JSON manifest document, and only then
tombstones the source. The destination manifest preserves the original
payload identity for decryption, checksums, encryption metadata, and every
replica location. Its Telegram filename is the extensionless send-attempt
token described above, not an indication that the content is non-JSON. When
the source tombstone later reaches evidence-first cleanup, the worker may also
publish a separate JSON deletion-evidence document; neither document is an
extra object payload. A selected folder is expanded from its local prefix
index, preserving its folder basename and relative descendants, including
directory markers. A partial move is recoverable: already committed
destinations remain visible and unprocessed sources remain intact for operator
review.

Automatic physical replication reuses a source chunk message when both
connected accounts point to the same Telegram storage chat. This creates a
ready target-account location without transferring payload bytes through the
server. Different storage chats retain the verified encrypted
download/upload fallback; access-mode replication remains a shared-chat
pointer by design.

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
that request is pending and leaves the last successful rows visible with a
loading affordance during the transition. Its silent multipart/activity poll
waits rather than superseding the listing request, so a slow Telegram-backed
response cannot leave the UI displaying an indefinite loading state. This affects
presentation only; manifest visibility and committed-object rules remain
unchanged.

The browser listing API returns bounded pages (`page`, `page_size`, `total`, and
`has_more`) for buckets and folder entries. Bucket search filters names; object
search recursively matches keys under the selected bucket/prefix and returns a
parent `location` for each match. The UI can jump from that location directly to
the containing folder, while download/share/link-manager/delete actions continue
to operate on the canonical full object key. Pagination and search do not change
manifest ordering, chunk references, or visibility rules. The admin listing
endpoints also accept validated sort keys and ascending/descending order; sorting
is applied to the complete metadata result before the requested page is sliced.
Folder entries remain name-ordered when an object-only column such as size or
modified time is selected because folders have no object size or modification
timestamp.

Bucket creation also protects the HTTP namespace: exact names `_public` and
`_admin` are reserved for public share links and the authenticated admin
surface. The shared object-format service rejects these names before metadata
creation, so S3 and admin callers cannot create a bucket that would be routed
away from the bucket API. This is a creation-time rule and does not rewrite
legacy metadata rows.

The browser also treats a stale cookie-bound CSRF token as a session-sync event:
it reads the current session and retries the rejected administrative request at
most once. This affects only control-plane request handling and does not alter
object manifests, chunk checksums, or Telegram storage layout.

The authenticated overview also reports payload telemetry in two tabs: `This
session` shows bytes received from and sent to clients plus bytes received from
and sent to Telegram since the current process started, while `Total` shows the
same four payload counters across all server runs. The all-time counters are
stored in metadata schema v14 and restored during service startup. Both views
refresh every five seconds, exclude protocol overhead, and do not participate
in manifest recovery or object visibility.

The same overview snapshot reports the logical size of unique committed
Telegram chunk payloads. It uses the remote chunk references in committed
manifests, counts a reused payload once, and intentionally excludes Telegram
protocol/message overhead; this is an operational estimate, not an account
quota or billing measurement.

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
