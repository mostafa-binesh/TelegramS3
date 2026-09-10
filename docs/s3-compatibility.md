# S3 Compatibility Matrix

Phase 3 has implemented the manifest/chunk object-format backend, and Phase 4
now wires the RustFS-backed S3 server through that layer for the CRUD slice.
Committed payloads are stored as Telegram documents/messages while SQLite
keeps the control-plane metadata and journal. The rows below track externally
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
| Get object | implemented | cargo test | Streams from Telegram-backed manifest and chunk references with checksum verification | Requires chunk fetch and verification | 4 |
| Head object | implemented | cargo test | Returns committed metadata only | Manifest rebuild may be needed | 4 |
| Delete object | implemented | cargo test | Tombstones before evidence-first cleanup | Telegram removal is asynchronous but due immediately | 4 |
| List objects v1 | implemented | cargo test | Uses the same ordered local manifest index and delimiter grouping as v2 so older clients can interoperate | Remote reconciliation lag exists | 4 |
| List objects v2 | implemented | cargo test | Uses the local index and manifest list for ordering | Remote reconciliation lag exists | 4 |
| Copy object | implemented | cargo check | Reuses the bounded object-format backend for source-to-destination copies | Copy is still local-first rather than remote-atomic | 5 |
| Byte-range GET | implemented | cargo test | Maps ranges to chunk spans and fetches only the required Telegram documents | Requires chunk-aware verification | 4 |
| Multipart initiation | implemented | cargo check | Persists durable upload state in the local metadata store | Multipart state is local | 5 |
| Multipart part upload | implemented | cargo check | Stages part data, uploads it to Telegram, and stores the returned identifiers | Each part must stay under Telegram limits | 5 |
| Multipart completion | implemented | cargo check | Commits manifest atomically after staged parts are verified and uploaded | Completion must reconcile staged parts | 5 |
| Multipart abort | implemented | cargo check | Marks upload aborted and cleans up local state | Abort is local cleanup | 5 |
| Multipart listing | implemented | cargo check | Lists live multipart sessions from the local journal/metadata | Telegram does not expose upload sessions natively | 5 |
| Conditional requests | implemented | cargo test | GET/HEAD/PUT and copy/delete preconditions honor ETag and timestamp guards | Requires strong object-state checks | 5 |
| Object versioning | implemented | cargo check | Version IDs are surfaced from manifests and version listings | Telegram lacks built-in versions | 5 |
| Delete markers | implemented | cargo check | Tombstones are listed as delete markers and remain recoverable until cleanup | Must be modeled locally | 5 |
| Object tags | compatibility gap | none yet | Must persist in manifest/index | Captions are not enough | 5 |
| Checksums | implemented | cargo test | Chunk and whole-object checksums are enforced during upload, read, and reconciliation | Telegram alone is not enough | 5 |
| Presigned URLs | compatibility gap | none yet | AWS SigV4 presigning is not implemented; the admin surface provides separate opaque `/_public/<token>` capability links | Share links are local metadata capabilities, not Telegram URLs | 5 |
| Server-side copy | implemented | cargo check | Copy uses the local object-format backend and manifest reuse | Telegram copy may not preserve metadata exactly | 5 |
| Lifecycle cleanup | implemented | cargo test | Garbage collection now removes only tombstoned data older than the 24-hour retention window after dry-run review | Cleanup is conservative and retention-based | 6 |
| Batch delete | compatibility gap | none yet | Can be translated to per-object tombstones | Telegram does not batch object deletes | 6 |
| Bucket policies | compatibility gap | none yet | Policy evaluation belongs above storage | Telegram is out of scope | 6 |
| Retention/object lock | compatibility gap | none yet | Requires additional metadata and enforcement | Telegram cannot enforce S3 locks | 6 |
| Event notifications | compatibility gap | none yet | Eventing is an upper layer concern | Telegram is not the notifier | 6 |
| Encryption | implemented | cargo test | Adapter-bound envelope encryption is keyed from `TELEGRAM_S3_MASTER_KEY` and recorded in manifests | Range semantics are bounded by chunk decrypt/read | 6 |
| Quotas | compatibility gap | none yet | Can be tracked locally | Telegram storage quotas are external | 6 |
| Metrics/health | implemented | cargo test | Loopback-only `/healthz` and `/metrics` endpoints report bootstrap and recovery state | Admin traffic stays off the S3 listener | 6 |

## Operator UI

- `/_admin` and `/_admin/api/*` are implemented as an authenticated operator
  surface served by the same Rust process.
- Login is credential-based: accounts are argon2id-hashed records in
  `metadata.sqlite` (schema v8), not the environment. A guest/operator sees only
  the sign-in screen; every management API requires a session bound to a user.
- Login is rate-limited with per-account lockout; passwords/session state are
  not stored in browser storage.
- The dashboard reports storage overview, endpoint details, capacity, Telegram
  readiness, operator accounts (superadmin-only add/remove), in-app bucket
  creation, and a bucket/object browser (prefix folders + directory markers +
  delete + per-file **upload/download**). Binary content is streamed through
  the same Telegram-backed object-format service and the shared bounded reader
  as the S3 data plane: uploads are
  `POST /_admin/api/objects/content?bucket&key`, downloads are
  `GET`/`HEAD` with an optional `Range` (`206`/`Content-Range`).
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
- Bulk/folder download or server-side ZIP is **not** available at this level
  (avoiding whole-object RAM buffering) and is an explicit future item.
- The bucket browser preserves bucket names exactly, including Unicode names;
  its delete action maps to the existing empty-bucket-only API and does not
  bypass tombstone/recovery rules.
- The object browser shows object expiry, accepts seconds-based expiry for
  browser uploads, and creates opaque `/_public/` share URLs with an optional expiry.
  Share URLs are public bearer capabilities bounded by object expiry.
- Removing the current Telegram connection from the admin UI is an explicit,
  CSRF-protected action that requires the linked Telegram phone number. Only a
  SHA-256 hash is stored for that confirmation. It hides local buckets, objects, and statistics
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
  a retry action rather than an unbounded loading skeleton.
