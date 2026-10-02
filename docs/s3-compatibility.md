# S3 Compatibility Matrix

Phase 3 has implemented the manifest/chunk object-format backend, and Phase 4
now wires the RustFS-backed S3 server through that layer for the CRUD slice.
Committed payloads are stored as Telegram documents/messages while SQLite
keeps the control-plane metadata and journal through a bounded eight-handle
connection pool. The rows below track externally
visible S3 API wiring; implemented entries are available through `server`, and
the standard S3 CRUD smoke test now passes. The authenticated operator frontend
and `/_admin` JSON API are operational surfaces, not S3 compatibility
features, so they are documented separately.

| API operation | Status | Test coverage | Compatibility notes | Telegram-specific limitation | Planned phase |
| --- | --- | --- | --- | --- | --- |
| Create bucket | implemented | cargo test | Creates the local bucket row and exposes it through the S3 server | Telegram has no native bucket primitive | 4 |
| Delete empty bucket | implemented | cargo test | Refuses non-empty buckets and preserves recoverable state until cleanup | Cleanup is asynchronous | 4 |
| List buckets | implemented | cargo test | Local index is authoritative for bucket visibility | Remote reconstruction is slower | 4 |
| Head bucket | implemented | cargo test | Reflects bucket metadata from the local store | Telegram metadata is indirect | 4 |
| Put object | implemented | cargo test / release-test.ps1 | Chunk upload plus manifest commit through the Telegram-backed object-format service; ambiguous sends are token-reconciled before safe retry, including restart recovery for stale sending attempts | 2 GiB Telegram file limit and bounded recovery scan | 4 |
| Per-object expiry (extension) | implemented | cargo test | PUT and multipart initiation accept `x-amz-meta-telegram-s3-expires-at` (RFC3339) or `x-amz-meta-telegram-s3-expires-in` (seconds); expired objects are hidden from reads and listings, then swept into tombstone cleanup | Background expiry sweep runs with the cleanup worker; remote deletion remains evidence-first and retention-aware | 11 |
| Get object | implemented | cargo test | Streams from Telegram-backed manifest and chunk references with checksum verification; transient Telegram reads retry per chunk while keeping the response open for up to 120 seconds; a bounded `0–4` prefetch policy supports adaptive parallel or sequential nearest-chunk scheduling without reordering output; an account-connection policy caps adaptive per-download concurrency by the enabled replica accounts and prefetch window, while one active payload read remains allowed per account; public/admin segmented ranges are serialized per client IP and object | Requires chunk fetch and verification; clients should resume with a byte range after an exhausted stream; higher prefetch, account limits, or failover retries can use more Telegram traffic | 4 |
| Head object | implemented | cargo test | Returns committed metadata only | Manifest rebuild may be needed | 4 |
| Delete object | implemented | cargo test | Tombstones before evidence-first cleanup | Telegram removal is asynchronous but due immediately | 4 |
| List objects v1 | implemented | cargo test | Uses the same ordered local manifest index and delimiter grouping as v2 so older clients can interoperate | Remote reconciliation lag exists | 4 |
| List objects v2 | implemented | cargo test | Uses the local index and manifest list for ordering | Remote reconciliation lag exists | 4 |
| Copy object | implemented | cargo check | Reuses the bounded object-format backend for source-to-destination copies | Copy is still local-first rather than remote-atomic | 5 |
| Byte-range GET | implemented | cargo test | Maps ranges to chunk spans and fetches only the required Telegram documents | Requires chunk-aware verification | 4 |
| Multipart initiation | implemented | cargo check | Persists durable upload state in the local metadata store | Multipart state is local | 5 |
| Multipart part upload | implemented | cargo test / Playwright | Stages part data, uploads it to Telegram, and stores the returned identifiers; a repeated in-flight `UploadPart` joins the original durable part job, while explicit Telegram `FLOOD_WAIT` replies pause and retry automatically | Each part must stay under Telegram limits; an intentional replacement waits until the active part job reaches a safe terminal state | 5 |
| Multipart completion | implemented | cargo test | Composes verified part chunk references, publishes only the final schema v2 manifest, and commits the object/session atomically | A pre-composition job with an ambiguous Telegram acknowledgement must still reconcile before replacement | 5 |
| Multipart abort | implemented | cargo check | Marks upload aborted and cleans up local state | Abort is local cleanup | 5 |
| Multipart listing | implemented | cargo check | Lists live multipart sessions from the local journal/metadata | Telegram does not expose upload sessions natively | 5 |
| Conditional requests | implemented | cargo test | GET/HEAD/PUT and copy/delete preconditions honor ETag and timestamp guards | Requires strong object-state checks | 5 |
| Object versioning | implemented | cargo check | Version IDs are surfaced from manifests and version listings | Telegram lacks built-in versions | 5 |
| Delete markers | implemented | cargo check | Tombstones are listed as delete markers and remain recoverable until cleanup | Must be modeled locally | 5 |
| Object tags | compatibility gap | none yet | Must persist in manifest/index | Captions are not enough | 5 |
| Checksums | implemented | cargo test | Ordinary PUTs use whole-byte SHA-256; multipart objects use `sha256-parts-v1`, a domain-separated digest of ordered part number, size, algorithm, and verified part checksum; every chunk is still SHA-256 verified on read | A multipart composite checksum is not the raw-byte SHA-256 of the concatenated object | 5 |
| Presigned URLs | compatibility gap | none yet | AWS SigV4 presigning is not implemented; the admin surface provides separate opaque `/_public/<token>` capability links | Share links are local metadata capabilities, not Telegram URLs | 5 |
| Server-side copy | implemented | cargo check | Copy uses the local object-format backend and manifest reuse | Telegram copy may not preserve metadata exactly | 5 |
| Lifecycle cleanup | implemented | cargo test | Garbage collection now removes only tombstoned data older than the configurable retention window, defaulting to 12 hours, after dry-run review | Cleanup is conservative, evidence-first, and retention-based; ambiguous evidence remains recovery-required | 6 |
| Batch delete | compatibility gap | none yet | Can be translated to per-object tombstones | Telegram does not batch object deletes | 6 |
| Bucket policies | compatibility gap | none yet | Policy evaluation belongs above storage | Telegram is out of scope | 6 |
| Retention/object lock | compatibility gap | none yet | Requires additional metadata and enforcement | Telegram cannot enforce S3 locks | 6 |
| Event notifications | compatibility gap | none yet | Eventing is an upper layer concern | Telegram is not the notifier | 6 |
| Encryption | implemented | cargo test | Adapter-bound envelope encryption is keyed from `TELEGRAM_S3_MASTER_KEY` and recorded in manifests | Range semantics are bounded by chunk decrypt/read | 6 |
| Quotas | compatibility gap | none yet | Can be tracked locally | Telegram storage quotas are external | 6 |
| Metrics/health | implemented | cargo test / Playwright | Loopback-only `/healthz` and `/metrics` endpoints report bootstrap and recovery state; the authenticated Overview caches manifest-derived counts and uses a cheap `/overview/live` payload for five-second transfer, traffic, stage, and connection refreshes; verifier duration and cleanup-state metrics are visible to operators | Admin traffic stays off the S3 listener; Overview storage values may be up to five seconds old | 6 |

## Operator UI

### Account pool and maintenance extensions

The operator API adds `/accounts`, `/replication`, `/replicas`, and `/rechunk`.
These are management extensions rather than S3 operations. Replication is
bucket-scoped and durable: `one_time` jobs finish once, while `automatic` jobs
are rescheduled. `replica` mode reuses the existing encrypted chunk message
when the source and target connections use the same Telegram storage chat,
avoiding server-side payload download/upload. When their storage chats differ,
it uploads encrypted chunk bytes to the target connection; `access` mode always
records a shared-chat location without uploading a second copy. A chunk may
have several account locations and the shared reader
rotates across them; an access-only target must be able to resolve the source
peer. The Accounts workspace is the single home for the primary connection and
additional connections, with separate Connections, Replication, and
Maintenance tabs. Bucket/object account badges open per-account copy/access
details and can queue either a whole-bucket or selected-key replication job.
Selected bulk actions remain available from the fixed bottom action bar.
The top-level bucket listing also supports selecting visible buckets and
guarded bulk deletion. Re-chunking is intentionally object-scoped: it queues
one durable job per selected object rather than treating a bucket as a single
file.

Each registered Telegram account has a durable `download_enabled` policy. A
disabled account is excluded from replica/access read selection while its
ownership, uploads, and evidence-first cleanup responsibilities remain intact.
The shared reader rotates across eligible primary and ready replica/access
locations per chunk. A failed selected account is retried according to the
`telegram_download_failover_retries` database setting before the next eligible
account is attempted; each attempt still uses the normal Telegram retry and
120-second stream-recovery policy.
Adaptive downloads also honor `telegram_download_account_connections` (schema
v24, default `5`, range `1–5`). For each object, the effective limit is the
smallest of the configured limit, the prefetch window, and the enabled account
locations recorded for that object. Sequential mode remains serial regardless
of the account limit.
Public-share and authenticated-admin streams add a narrower fairness gate: for
one client IP and one object, only one Telegram payload read is active at a
time. IDM-style ranges with different starting offsets therefore wait for the
current chunk instead of multiplying Telegram traffic; different objects are
not coupled.
The Overview aggregates account health as connected when all configured
accounts are connected, partial when only some are connected, and disconnected
when none are connected; individual account indicators expose the account
label on hover.
The Accounts/Connections panel performs one initial account state load and
refreshes account data only when the operator clicks its explicit refresh
button; it does not poll the account list in the background.

An expired admin session is treated as an authentication state transition: the
SPA refreshes `/session` after a `401` and returns the operator to login. It
does not leave the operator on a stale request-error view.

Bulk re-chunking gives each selected object a durable job and temporary lock.
Selecting bucket rows expands to every committed object in those buckets before
the jobs are queued; selecting object rows queues only those objects. Reads
fail closed with a retry-later message until replacement chunks and the manifest
are committed. The previous manifest remains recoverable until the normal
evidence-first cleanup worker handles it. The operator chooses whether the job
should apply to replicas. When enabled, the job snapshots the old physical and
access-only account targets in schema v18 and queues durable follow-up jobs for
the replacement manifest after the primary commit. When disabled, old replica
locations remain attached only to the old manifest and are cleaned up with it;
the replacement is therefore primary-only until it is replicated again.
The additional-account wizard saves its account definition first, then sends
`account_id` with every login flow so the code/password steps use that account's
session and transport rather than the primary connection.

The sampled integrity verifier checks the primary and every recorded account
location for each sampled chunk. A missing, corrupt, or checksum-bad physical
replica is logged with its account and chunk, then repaired by uploading the
verified encrypted bytes from another healthy location when possible. A
replica-only failure does not hide an object whose primary or another replica
is healthy; only an unrecoverable primary failure moves the object to
`recovery_required`. Repaired findings remain visible in the Recovery history,
while transient account/network failures remain retryable.

- `/_admin` and `/_admin/api/*` are implemented as an authenticated operator
  surface served by the same Rust process.
- Login is credential-based: accounts are argon2id-hashed records in
  `metadata.sqlite` (schema v8), not the environment. A guest/operator sees only
  the sign-in screen; every management API requires a session bound to a user.
- Login is rate-limited with per-account lockout; passwords/session state are
  not stored in browser storage.
- The admin SPA resynchronizes its cookie-bound CSRF token after the specific
  `invalid csrf token` response and retries that action once. This is an
  operator-console convenience only; the server still requires the signed
  session claim and matching CSRF header for every mutating endpoint.
- The dashboard reports storage overview, endpoint details, capacity, live
  transfer-pipeline analytics, five-second client/Telegram traffic telemetry
  with session and all-time views, system checks, storage safeguards, Telegram
  readiness, sampled recovery-verifier enabled/disabled state, timing, and
  broken-file problems,
  plus the logical size of unique committed Telegram chunk payloads (reused
  multipart references are counted once and protocol overhead is excluded),
  and a bounded Download stage metrics testing section that separates first
  chunk, Telegram, retry-wait, decrypt, checksum-verify, and total read time
  for recent public/admin/S3 streams and shows client/Telegram byte totals;
  completed final-byte delivery is not mislabeled as cancellation when a
  response consumer closes without an extra terminal poll,
  and a Storage policy action that manually queues the integrity verifier and
  resets its next scheduled interval after the run completes,
  plus a safe **Run eligible cleanup now** action that wakes the durable worker
  without bypassing retention windows or recovery-required quarantine,
  operator accounts (superadmin-only add/remove), in-app bucket
  creation, and a bucket/object browser (prefix folders + directory markers +
  delete + per-file **upload/download**). Binary content is streamed through
  the same Telegram-backed object-format service and the shared bounded reader
  as the S3 data plane: uploads are
  `POST /_admin/api/objects/content?bucket&key`, downloads are
  `GET`/`HEAD` with an optional `Range` (`206`/`Content-Range`).
- The authenticated browser API supports bounded listing pages through
  `page`/`page_size` plus `total`/`has_more` metadata. `GET /_admin/api/buckets`
  accepts `search` for bucket-name filtering; `GET /_admin/api/objects` accepts
  `search` for recursive key matching within the selected bucket/prefix. Search
  results include a parent `location` and preserve download, share, link-manager,
  delete, and direct-go-to-folder actions. Both listing endpoints accept the
  operator-console sort keys and `order=asc|desc`, applying sorting before
  pagination so page changes preserve the global order. These are
  operator-console features;
  S3 `ListObjects` and `ListObjectsV2` semantics are unchanged.
- Object, folder, and bucket rows expose applicable maintenance and destructive
   operations through a responsive Actions menu, while the fixed bulk bar keeps
   its multi-selection actions. A row-level re-chunk or move carries an isolated
   target scope and does not reuse an unrelated selection. Shared-link expiry
   editing is a per-link expandable control; it changes presentation only and
   still uses the existing authenticated link-management API.
- Each object row also exposes an on-demand Information panel. The authenticated
   `GET /_admin/api/objects/details?bucket&key` endpoint returns the committed
   manifest summary, checksum/encryption policy, user metadata and tags, and
   bounded chunk/replica placement details; it is not fetched during ordinary
   bucket listing.
- Path-style bucket names `_public` and `_admin` are reserved because those
  paths dispatch to the public-share and authenticated-admin HTTP surfaces.
  `CreateBucket` rejects either exact name with the standard
  `InvalidBucketName` error; the admin JSON endpoint returns HTTP 400. Existing
  metadata rows are not rewritten by this rule.
- The bucket browser also reads active transfer jobs for the selected bucket and
  prefix. An S3 multipart key appears as an in-progress row while its final
  manifest is being committed, with an accessible percentage bar, completed/
  total part count, and separate receiving, uploading, finalizing, and
  needs-attention presentation; recovery-required rows remain non-downloadable
  until repaired.
- The operator UI hosts an in-browser **Telegram account wizard**
  (`/telegram/wizard/{state,begin,submit-code,submit-password,cancel}`) that
  configures API credentials, storage chat, and network settings before driving
  the real single-account login (phone → code → cloud password when required)
  behind the authenticated, CSRF-protected session. The inline wizard sends an
  opaque `flow_id` on every turn, replaces only the same operator's unfinished
  flow, and preserves retryable invalid-code state. This authorizes
  the storage account for the server, not an operator record in the dashboard.
- Telegram bootstrap settings are validated by the admin API before they are
  persisted, so malformed API/storage identifiers remain operator errors and
  do not change the S3 compatibility contract.
- The admin browser upload surface also exposes reception-only resumability:
  `POST /_admin/api/uploads/resumable`, `GET` status,
  `PATCH /_admin/api/uploads/resumable/{id}?offset=N&final=0|1`,
  `POST .../{id}/complete`, and `DELETE .../{id}`. Chunks are encrypted into
  the existing staging area before the durable transfer is queued. These are
  authenticated admin-plane endpoints, not S3 multipart APIs; a server restart
  drops an in-flight browser reception and the existing inactivity lease still
  applies.
- The authenticated admin object browser supports server-side moves for
  selected files and folders through `POST /_admin/api/objects/move`.
  Destination manifests reuse the existing encrypted chunk references and are
  committed before source tombstones, so neither the browser nor the server
  re-uploads object payloads. Folder moves preserve the selected folder name
  and recursively include descendants and directory markers. This is an
  admin-plane operation and does not add an S3 MoveObject API.
- Bulk/folder download or server-side ZIP is **not** available at this level
  (avoiding whole-object RAM buffering) and is an explicit future item.
- The bucket browser preserves bucket names exactly, including Unicode names;
  its delete action maps to the existing empty-bucket-only API and does not
  bypass tombstone/recovery rules. Object deletion hides the object only after
  the local tombstone is committed; missing objects return `404`, and folders
  with active descendants return `409` instead of a false success.
- Public share and authenticated admin downloads advertise
  `Accept-Ranges: bytes` and honor single byte ranges. A client can resume a
  disconnected download using `Range` and `Content-Range`; the shared reader
  retries transient Telegram/network failures while fetching the current chunk
  and keeps a bounded number of additional verified chunks ready in order
  while keeping the response open for up to 120 seconds. Missing messages,
  decryption failures, and checksum failures are not retried.
  Segmented ranges for the same client IP and object are serialized at the
  Telegram-read stage, while range responses remain resumable and ordered.
- Public `audio/*` and `video/*` share responses use inline content disposition
  with the stored media type, which lets HTTP media players probe and play the
  link. Other public objects remain attachment downloads; this does not alter
  S3 object bytes or range semantics.
- The object browser shows object expiry, accepts seconds-based expiry for
  browser uploads, and creates opaque `/_public/` share URLs with an optional expiry.
  Share URLs are public bearer capabilities bounded by object expiry. Each row
  shows a shared-link count, and the authenticated link manager can list
  descriptions, copy URLs, update expiry, and revoke links. This is an admin
  control-plane feature, not an AWS presigned URL implementation.
- Removing the current Telegram connection from the admin UI is an explicit,
  CSRF-protected action that displays the linked Telegram account phone number
  and requires typing that exact value as confirmation. The phone number is
  stored in local metadata for this account-identity prompt. It hides local buckets, objects, and statistics
  immediately. The optional "delete uploaded files" choice queues Telegram
  message/document cleanup in the durable worker and reserves the owning
  generation/transport until cleanup completes; targets cannot be processed by
  a later login. Without that choice, the connection is detached immediately
  and remote Telegram files are intentionally retained and are no longer
  managed by this installation. Recovery-required files and cancelled transfer
  rows owned by the removed generation are removed from the attention views and
  local operational state. The wizard reports success only when the same
  end-to-end health check used by the overview can resolve the storage peer.
- The operator UI is not part of the S3 compatibility contract; the `/_admin`
  controller only reflects committed S3 object data through the same store as
  the S3 server. Its Vite-built hashed chunks are served from the UI dist
  directory's `assets/` subdirectory; a failed lazy-view load is surfaced with
  a retry action rather than an unbounded loading skeleton. Folder transitions
  keep the last successful listing visible with a loading affordance while the
  foreground object request remains authoritative and the background poll waits
  for it, so slow admin responses do not strand the browser in a loading state.
