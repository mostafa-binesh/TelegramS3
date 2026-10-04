# Roadmap

## Phase 0 - Upstream analysis

- Status: complete
- Exit criteria:
  - upstream commit hashes recorded
  - RustFS persistence path mapped
  - Telegram Drive reusable backend pieces identified
  - integration strategy recorded in ADR 0001

## Phase 1 - Project foundation

- Status: complete
- Exit criteria:
  - workspace scaffold exists
  - configuration and redaction code exists
  - fake Telegram client exists
  - metadata schema and migrations exist
  - documentation set is populated

## Phase 2 - Telegram transport

- Status: complete
- Exit criteria:
  - headless login flow exists
  - session persistence works
  - direct and SOCKS5 proxy support works
  - retries and flood waits are handled explicitly

Completed work:

- headless auth/login/status/logout flow is wired through the transport service
- session reuse is persisted through a system-derived Telegram session path
- proxy resolution covers direct, SOCKS5, and bridged HTTP/HTTPS modes
- a shared retry and flood-wait policy is used by transport calls
- smoke tests cover the transport bootstrap path in mock mode

## Phase 3 - Object format

- Status: complete
- Exit criteria:
  - manifest and chunk format implemented
  - checksums verified
  - operation journal and reconciliation implemented

Completed work:

- the object-format service now chunks uploads, writes staged manifests, and commits only after checksum verification
- bounded range reads map manifest chunk spans back to Telegram-backed chunk references
- startup reconciliation repairs complete staged uploads, marks incomplete objects recovery-required, and quarantines orphaned data
- committed object payloads now live in Telegram documents/messages while SQLite keeps the control-plane index and journal
- `doctor` and `server` now fail fast if object-format bootstrap finds unresolved recovery state
- server listeners bind before the first Telegram-backed recovery snapshot; the background worker publishes recovery visibility after startup instead of blocking the container on remote chunk reads
- local metadata uses a bounded eight-handle SQLite pool with WAL, busy-timeout
  protection, and replacement of isolated low-level connection failures; this
  is runtime-only and does not require a schema migration

## Phase 4 - RustFS integration

- Status: complete
- Exit criteria:
  - chosen RustFS seam is implemented
  - bucket/object CRUD vertical slice works
  - range reads and listings are proven with a standard S3 client

Completed work:

- the RustFS-backed `server` bootstrap now starts the S3 listener instead of only validating configuration
- bucket CRUD and object CRUD are routed through the RustFS seam into the Phase 3 object-format service
- both legacy `ListObjects` and `ListObjectsV2` now read from the same ordered local manifest index, improving compatibility with older S3 tools
- `doctor` now exercises the same server bootstrap path so Telegram/session and seam mismatches fail fast
- standard S3 client smoke tests now cover create, put, head, get, range-get, list, and delete flows against the temporary server

## Phase 5 - Multipart and advanced compatibility

- Status: complete
- Exit criteria:
  - multipart recovery is durable
  - compatibility matrix is updated
  - crash and fault injection coverage exists

Completed work:

- multipart sessions and parts now persist in the local metadata store
- multipart initiate, upload-part, upload-part-copy, complete, abort, and list flows are wired through the S3 server seam
- multipart completion composes immutable part chunk references into the final
  schema v2 manifest, uploads only that manifest, and preserves source
  encryption identities without a whole-object Telegram round trip
- final cleanup removes private part manifests but retains every chunk now
  referenced by the committed object
- copy-object and version-aware listings now flow through the same object-format backend
- checksum enforcement now runs through part uploads, chunk reads, and reconciliation
- conditional requests now honor ETag and timestamp preconditions on read, write, copy, and delete paths
- the roadmap and storage docs now reflect the shipped behavior

## Phase 6 - Production hardening

- Status: complete
- Exit criteria:
  - encryption, repair, garbage collection, metrics, and deployment docs are complete
  - recovery drills and benchmark plan are in place

Completed work:

- adapter-bound envelope encryption is wired through the manifest and object format with metadata-recorded encryption state
- repair and garbage collection now run against local metadata with dry-run support and conservative cleanup gating
- `/healthz` and `/metrics` are served from a localhost-only admin listener separate from the S3 listener
- the roadmap, operations docs, storage docs, disaster-recovery docs, and phase-6 ADR are aligned with shipped behavior

## Phase 7 - Docker packaging and deployment

- Status: complete
- Exit criteria:
  - the project ships a production Docker image with reproducible builds and a clear release tag flow
  - docker-compose covers the main server only, with bootstrap handled in-process rather than by a separate setup service
  - persistent state is mounted cleanly for metadata, chunks, manifests, sessions, and any upload staging data
  - runtime configuration is driven by environment variables and documented secrets mounts
  - deployment docs explain S3 exposure, localhost-only admin exposure, restart behavior, and backup/restore expectations
  - Docker smoke checks prove build, startup, health probing, and a basic S3 CRUD path

Completed work:

- a multi-stage Dockerfile now builds the release binary and packages it into a minimal runtime image
- a single-service `docker-compose.yml` now mounts metadata, data, and session volumes and exposes only the S3 listener
- the container entrypoint validates config before foreground server startup without adding a separate setup service
- docker-oriented deployment and recovery steps are documented in the operations and configuration guides
- packaging smoke tests cover the Docker assets and a `docker compose config` validation path

## Phase 8 - Authenticated operator frontend

- Status: complete
- Exit criteria:
  - an embedded high-performance frontend provides storage overview, endpoint details, capacity, and bootstrap status
  - the admin surface is protected by an authentication barrier before exposing operational data
  - the account setup and Telegram onboarding flow live behind the same authenticated app instead of a separate docker-compose service
  - the frontend can guide first-run setup for phone number, `.env` values, 2FA, and connection checks
  - no sensitive state is exposed before authentication succeeds

Completed work:

- the Rust server now serves an authenticated `/_admin` SPA and JSON API on the existing public listener
- login originally used a bootstrap-secret gate; as of Phase 9 it is replaced by credential (username/password) sessions
- the dashboard surfaces storage overview, endpoint details, capacity, bootstrap status, and Telegram readiness
- the onboarding panel gives operators a guided checklist for phone number, `.env` values, 2FA, and connection checks
- the Docker image builds the Svelte frontend in a dedicated build stage and copies the runtime assets into the single container

## Phase 9 - Multi-user control plane

- Status: in progress (initial control-plane slice + the bounded-content-streaming and in-browser Telegram-wizard increment landed)
- Exit criteria:
  - multiple operator accounts are supported as database-backed username/password records (no per-user `.env`)
  - account management (add/list/delete/password change) is administered by a superadmin in-app
  - guests/unauthenticated visitors to `/_admin` see only a login screen; every data API is gated
  - login is rate-limited/locked out per account; sessions are bound to a user and revocable (password change / user delete / logout)
  - the CLI can provision the first superuser while the server is down (`telegram-s3 users create`)
  - boundaries are documented: all accounts are admin-tier today (`role` reserved for future per-user scopes/tenants)

Completed work (initial slice):

- real username + password accounts stored in `metadata.sqlite` with argon2id-hashed passwords (new `auth` module), plus admin-managed Telegram bootstrap settings in schema `v5`
- signed HTTP-only session cookies bound to a `admin_sessions` row; logout revokes that row; password change / user delete revoke all of that user's sessions via `token_version`
- `/_admin/api/session` (whoami, guest-safe), `/session/login`, `/session/logout`, `/session/refresh`, with CSRF on mutating endpoints
- user management API: `GET/POST /users`, `DELETE /users/{id}`, per-user password change; superadmin gets account CRUD
- in-browser management UI: username/password sign-in, overview, operator list, in-app bucket creation, and a (JSON) bucket/object browser with prefix folders + directory markers + file/folder delete
- CLI `users` family (`create`, `list`, `password`, `delete`, `status`); first account is forced to superadmin and is provisionable while the server is down
- login rate limiting / lockout (in-process per-IP + per-account buckets)
- self-service operator profile page with atomic display-name updates, current-
  password-verified password rotation, session revocation, and browser
  validation/error coverage
- unit + integration smoke coverage for auth, migrations, user CRUD, and the credential login lifecycle

Next slice (landed this increment): bounded binary content streaming over `_admin` and the in-browser Telegram onboarding wizard.

Completed work (content streaming + wizard increment):

- bounded file upload (`POST /_admin/api/objects/content?bucket&key`, raw body, CSRF) reuses the S3 `put_stream` data-plane writer so nothing is buffered in RAM
- full + ranged download answering over the same authenticated surface: `GET`/`HEAD /_admin/api/objects/content` with range requests returning `206` + `Content-Range`, correct `ETag`/`Content-Length`/`Content-Disposition`
- both the S3 `get_object` and the admin download now feed a single shared chunk reader (`ObjectFormatService::read_spans_to_stream`), so byte-for-byte S3 output is preserved and memory stays bounded per chunk
- in-browser Telegram onboarding wizard behind the shared live transport manager: authenticated `/telegram/wizard/{state,begin,submit-code,submit-password,cancel}` with a staged, single-in-flight driver (second begin → `409`), mock-runtime test path, and 2FA (cloud-password) stage surfaced only when Telegram asks for it
- successful Telegram reauthorization refreshes the live transport immediately, and the overview card now reports connected / disconnected / needs reauth from the same runtime snapshot
- the Svelte UI gains per-file upload + progress, per-row Download, in-app bucket creation, and a three-step Telegram set-up flow; readiness panel now reflects the storage connection state directly
- Telegram settings saves validate numeric API/storage identifiers before persistence and report refresh failures as JSON warnings instead of surfacing as proxy 502s
- object expiry is persisted in rebuildable manifests, configurable from the
  admin upload UI or S3 extension headers, and enforced on S3/admin reads and
  listings
- the admin object browser can issue opaque `/_public/<token>` download links
  with optional expiry capped by the object's expiry; public links use the
  bounded shared reader
- the bucket browser now shows a shared-link count badge and a link manager
  modal with descriptions, URL copy, expiry editing, expired-link visibility,
  and revocation; token ciphertext is encrypted for post-creation management
- bucket and operator lists use icon actions with hover labels, destructive
  actions use in-app confirmation modals, share expiry is collected in-app,
  and Telegram connection removal requires linked-phone confirmation

Remaining Phase-9 follow-ups (explicitly out of this increment, see ADR-0006 / ROADMAP): bulk/folder download or server-side ZIP (no whole-RAM buffering), drag-in of nested directory trees, and browser resumable-multipart upload negotiation.

Rejected alternatives this phase (see `docs/adr/0006-...md`): keeping MinIO-time `TELEGRAM_ADMIN_BOOTSTRAP_SECRET` as a shared login secret; per-user `.env` accounts; a separate credentials SQLite file; `governor`-style thundering rate limiters; site-replication peering of Telegram S3 (documented unsupported).

## Phase 10 - Durability and frontend refresh

- Status: in progress
- Exit criteria:
  - object uploads are protected by a durable background workflow and can survive restart, retry, and partial-failure cases without losing staged chunks
  - the operator UI is redesigned around a modern login-first flow with a first-run superadmin wizard
  - the dashboard shows live Telegram connection health at a glance
  - recovery, repair, and upload-failure states are explainable from the UI and docs

Completed in this increment:

- admin actions recover once from a stale CSRF token by synchronizing the
  cookie-bound session before retrying, including raw and resumable uploads;
  server-side CSRF validation remains unchanged

- the admin console now uses full-width responsive layout, loading skeletons,
  animated dismissible notifications, recovery sub-tabs, and analysis cards for
  storage composition, transfer pipeline, system checks, storage safeguards, and
  client/Telegram traffic refreshed every five seconds, with This session and
  metadata-backed all-time Total views
- browser uploads are serialized, retried after transient request failures, and
  follow the durable transfer job through chunk progress in the upload queue
- the admin console now has history-routed bucket/folder locations, SVG
  navigation icons, a folder-aware move browser, and Telegram connection/proxy
  sub-tabs
- bucket and object browsing now use bounded server-side pages, searchable
  bucket names, recursive object search, result location breadcrumbs, direct
  parent-folder navigation, sortable bucket/object columns across paginated
  results, and the existing per-object actions for search results
- `_public` and `_admin` are reserved at the shared bucket-creation boundary;
  S3 clients receive `InvalidBucketName` and the admin form receives a clear
  validation error
- the Telegram settings page now exposes a database-backed Storage policy tab;
  chunk size can be changed live for new uploads while existing manifests and
  active receptions retain their captured boundaries
- the Storage policy tab now controls a sampled recovery verifier: its interval
  and random chunk count are persisted live, it can be disabled completely, the
  overview shows its state, next run, and distinct broken-file count, and
  verifier problems are listed with their durable recovery state
- lazy-loaded console views now surface a retryable asset-load error instead of
  remaining on an infinite skeleton; Docker/static serving keeps hashed Vite
  chunks under `/_admin/assets/`
- public, authenticated admin, and S3 downloads now share a 120-second
  transient Telegram/proxy recovery window per chunk, log exhausted stream
  failures with object/chunk context, and advertise byte-range resume support
  so clients can continue after a disconnect without restarting the complete
  object
- download streams now support a bounded, database-backed parallel prefetch
  window of `0–4` extra verified chunks, configurable from Telegram settings;
  output order, checksum boundaries, and memory use remain bounded
- the prefetch window is adaptive: it ramps up after clean reads and backs off
  after retries or measured throughput drops, while a per-account limiter keeps
  one account from competing with itself
- storage policy now offers a sequential nearest-chunk mode that preloads the
  configured look-ahead one chunk at a time in manifest order
- the shared download reader now delivers the first requested chunk before
  opening speculative prefetch, reducing time to first byte without changing
  the ordered `0–4` concurrency bound
- storage policy now limits adaptive parallel downloads to a configurable
  `1–5` account connections, automatically capped by each file's enabled
  replica locations and prefetch window; one active payload read remains
  allowed per account
- the Overview includes a bounded Download stage metrics testing section for
  recent public/admin/S3 reads, exposing first-chunk, Telegram, retry-wait,
  decrypt, checksum-verify, and total timings without adding per-read SQLite
  writes; it also reports client/Telegram byte totals, adaptive-window values,
  and per-account Telegram bytes/retries/duration, and treats complete
  final-byte delivery as completed when a response consumer closes early
- active reads expose process-local object, mode, chunk, client-byte,
  Telegram-byte, retry, and server/client chunk progress without per-chunk
  SQLite writes, including the effective account-connection limit and eligible
  replica-account count
- S3, public-share, and authenticated-admin segmented range requests use a
  per-client/object admission gate held for the full response, preventing
  IDM-style range fan-out from creating parallel stages or Telegram reads
  while keeping different objects independent; extra client sockets may wait
  because HTTP cannot force the client to open only one socket
- the Overview caches manifest-derived object counts and unique Telegram
  payload size for five seconds; its five-second refresh uses the cheap
  `/overview/live` endpoint for transfer, traffic, stage, and connection
  telemetry instead of rescanning manifests
- verifier scans now log start/end or failure, record duration and run/failure
  counters, and expose those metrics in the Overview; cleanup counts are
  separated into due, scheduled, and recovery-required work; deferred startup
  scans expose their scheduled first-run time before the first scan completes
- cleanup claims now use dependency indexes and idle backoff instead of
  rescanning the full outbox every second; cleanup evidence attempts have
  durable unique tokens for exact reconciliation, and future delayed cleanup
  retention is configurable from Storage policy with a 12-hour default
- public audio/video links now advertise inline media disposition while
  non-media links remain attachments, preserving range-based player probing
  without changing stored objects
- the Overview reports the logical size of unique committed Telegram chunk
  payloads without counting reused multipart references twice
- browser reception now supports bounded resumable chunks, authoritative offset
  re-sync, pause/resume while the reception lease remains active, and explicit
  cancellation cleanup; server restart resumption remains intentionally
  unsupported
- committed-object deletes now make cleanup due immediately; the existing
  evidence-first cleanup worker removes Telegram messages asynchronously
- connection removal hides the local namespace immediately but retains the
  owning Telegram generation and transport until evidence-first remote cleanup
  completes; generation-tagged targets cannot be deleted through a newly
  logged-in account, and the owning connection is explicitly reserved during
  `remote_cleanup_pending`
- Telegram health and the login wizard now share the end-to-end readiness
  contract: `authorized` is not shown as connected when storage-peer lookup
  fails, including `AUTH_KEY_UNREGISTERED`; `TELEGRAM_SESSION_PATH` is honored
  consistently by runtime bootstrap
- the Telegram account view now uses an accessible inline four-step wizard
  (API credentials, storage chat, network, and sign-in) with an opaque,
  operator-owned flow id; reopening replaces the operator's unfinished attempt,
  while other operators remain isolated by the single-flow lock
- invalid Telegram codes remain retryable, and the bucket browser now provides
  empty-bucket deletion, parent navigation, and skeleton loading on refresh;
  folder transitions keep the last successful listing visible with a loading
  affordance while the foreground request remains authoritative and silent
  background polling waits, so a slow response cannot leave the browser on an
  unbounded loading skeleton
- bucket/object deletion now reports only committed local deletes, returns a
  conflict for non-empty folders, and keeps folder and file action columns
  aligned in the browser table
- the account view can remove the current Telegram connection behind an
  explicit confirmation; local buckets, objects, and statistics are hidden
  immediately, while optional Telegram payload deletion is a durable,
  restart-safe worker job
- connection-owned transfer, multipart, manifest, and bucket records now
  carry immutable generation ownership; removing a connection clears its
  recovery/attention rows and local operational records without affecting
  another connection's records
- Telegram sends now have durable attempt tokens and diagnostics; ambiguous
  acknowledgements are automatically reconciled by token plus exact encrypted
  bytes, and unmatched sends are safely retried after a complete history scan
- explicit Telegram `FLOOD_WAIT` replies are classified as safe
  pre-publication rejections, so the worker observes Telegram's pacing and
  resumes automatically; historical flood-wait rows retain that delay even
  when another genuinely ambiguous send must be reconciled first
- repeated in-flight `UploadPart` calls now join their canonical durable part
  job rather than creating duplicate Telegram uploads; only duplicates with no
  checkpointed or ambiguous remote send can be discarded after the part commits
- the bucket browser now merges active S3 transfer jobs into the object listing,
  showing an accessible progress card with receiving, uploading, waiting for
  Telegram, finalizing, and needs-attention phases while the final multipart
  manifest is still hidden; a short activity-endpoint outage retains the last
  visible row instead of making it fade away
- multipart completion regression coverage verifies metadata-only composition,
  cross-part range reads, retained-message cleanup safety, idempotency, and
  restart reads

Multi-user boundary for the next increment:

- the current process still supports one active Telegram connection, but all
  new lifecycle work must preserve immutable connection/account ownership
  boundaries
- split bootstrap settings, session paths, storage-chat bindings, transport
  handles, and cleanup credentials by account/connection before allowing a
  second Telegram account to log in concurrently
- add two-account isolation, restart, and cross-account cleanup tests before
  changing the single-connection reservation into concurrent cleanup

### Phase 11 - Account pool, replication, and maintenance (rc.11)

- [x] Additive schema migration for multiple Telegram connections and durable
  replication/re-chunk job state.
- [x] Schema v16 preserves existing replication jobs while adding durable
  selected-object scopes for bulk replication.
- [x] One-time and automatic bucket replication controls with physical-copy and
  access-only modes.
- [x] Replica/access badges, account detail inspection, and round-robin chunk
  reads across ready physical replicas.
- [x] Bulk re-chunk queue with progress and temporary object locks.
- [x] Browser and migration coverage for the operator paths, including expired
  session redirect, staged diagnostic tests, account tabs, replica details,
  fixed bulk actions, and recursive search location navigation.
- [x] Refactored account management into selectable account cards with one
  add/edit form, including persisted per-account download eligibility.
- [x] Added aggregate Telegram health (`connected`, `partial`, or
  `disconnected`) with per-account status indicators and cached account
  transports for overview and replica reads.
- [x] Widened folder table geometry so modified timestamps and action controls
  remain separate at desktop widths, with browser regression coverage.
- [x] Added visible-bucket checkbox selection, select-all-visible behavior, and
  fixed-bottom bulk deletion/re-chunk actions; bucket re-chunking expands to
  every committed object while object-row re-chunking remains supported.
- [x] Added responsive per-row Actions menus with parity for applicable bulk
   operations, including server-side move, replication, re-chunking, and guarded
   deletion; single-row maintenance targets no longer overwrite another bulk
   selection.
- [x] Added an on-demand object Information panel and authenticated details
   endpoint for manifest identity, object policy, checksums, chunk layout, and
   replica locations, with successful and error-path browser coverage.
- [x] Reworked the shared-link manager's expiry controls into per-link
  expandable disclosures with coverage for link sets larger than two entries.
- [x] Added an isolated additional-account onboarding wizard that carries its
  account ID through every Telegram login step and leaves primary settings
  untouched.
 - [x] Added schema v18 durable re-chunk replica policy and target snapshots;
   operators can rebuild prior replica/access targets or choose primary-only
   re-chunking with explicit read-availability consequences.
 - [x] Fixed re-chunk staging order and final-chunk progress accounting so a
   queued job can publish its requested layout instead of failing before the
   first chunk.
- [x] Added replica source chunk-size metadata and object/bucket mismatch
  reporting, with a yellow account badge when a persisted replica layout differs
  from the canonical manifest layout; legacy unknown values remain readable.
- [x] Added schema v19 database-backed account failover retries (`0–8`, default
  `1`) with complete per-chunk retries before rotation to the next enabled
  replica account, plus settings and browser coverage.
- [x] Added schema v20 durable integrity-recovery events. Sampled verifier
  checks now cover every recorded account location, identify the affected
  account/chunk, repair damaged physical locations from a verified alternate,
  isolate unrecoverable replica-only locations from reads, and quarantine only
  an unrecoverable primary. Added Rust and browser coverage for repaired replica
  history and migration/persistence.
- [x] Removed background polling from the Accounts/Connections panel and added
  an explicit manual refresh action with browser coverage for the no-poll and
  on-demand request paths.
- [x] Replaced browser-mediated object Move with an authenticated durable
  manifest-only transfer that reuses encrypted chunk references, commits each
  destination before tombstoning its source, and never re-uploads payloads.
  Folder rows are selectable and move recursively while preserving directory
  markers; browser request and failure-boundary coverage was added.
- [x] Optimized automatic physical replication when connected accounts share a
  Telegram storage chat by reusing existing chunk messages; different chats
  retain the verified encrypted download/upload fallback.
- [x] Added schema v22 for the persisted first-remote-scan-on-startup policy.
  The Storage policy UI can defer that first scan without disabling recurring
  verifier scans, with migration and browser coverage.
- [x] Added a manual Storage policy verifier action. It queues the same worker
  scan immediately, resets the next interval after completion, rejects runs
  while verification is disabled, and has Rust and Playwright coverage.
- [x] Added a manual eligible-cleanup action. It wakes the durable cleanup
  worker without bypassing retention windows, evidence-first ordering, or
  `recovery_required` quarantine, with Rust and Playwright coverage.
- [x] Fixed access-replica reads to resolve the persisted source peer together
  with each Telegram message ID across public, admin, range, prefetch, failover,
  verifier, and replication-read paths; added a disabled-primary regression.

Remaining Phase 10 work:

- release-grade deterministic fault/restart coverage now includes the persisted
  `recovery_required` + null-lease + `sending` regression, exact-byte mock
  reconciliation, corruption rejection, and a repeatable PowerShell release
  runner; the isolated live Telegram drill remains a separate RC/stable gate
- expand fault-injection coverage for Telegram timeouts, process crashes, and
  history-scan limits beyond the current deterministic matrix
- continue browser coverage beyond the now-covered multipart upload/finalizing/
  recovery progress states into transfer retry/cancel, wizard, and degraded
  recovery flows
- add periodic reconciliation for staged uploads so missing chunks, orphaned
  staging trees, and interrupted commits are repaired or quarantined before
  they become user-visible corruption
- make the startup path fail soft for recoverable upload issues: the app should boot, expose health, and let operators inspect or repair state instead of disappearing when staging is damaged

Notes:

- `100 staged chunk(s) missing` is a durability symptom, not just a UI issue; the long-term fix is a worker-backed upload lifecycle plus reconciliation, not only a prettier error message.
- object and connection-removal cleanup is worker-backed: tombstone first, then
  let durable cleanup jobs remove Telegram payloads later, so deletes stay
  recoverable and retryable; local tombstones and orphaned cleanup material are
  retained for 24 hours before irreversible garbage collection.
- the redesigned UI should keep health and recovery visible even when Telegram bootstrap is degraded, so operators can fix settings without losing the whole control plane.
