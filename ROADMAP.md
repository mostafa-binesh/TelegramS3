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

- the admin console now uses full-width responsive layout, loading skeletons,
  animated dismissible notifications, recovery sub-tabs, and analysis cards
- browser uploads are serialized, retried after transient request failures, and
  follow the durable transfer job through chunk progress in the upload queue
- the admin console now has history-routed bucket/folder locations, SVG
  navigation icons, a folder-aware move browser, and Telegram connection/proxy
  sub-tabs
- lazy-loaded console views now surface a retryable asset-load error instead of
  remaining on an infinite skeleton; Docker/static serving keeps hashed Vite
  chunks under `/_admin/assets/`
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
- the Telegram setup wizard now uses an accessible modal with an opaque,
  operator-owned flow id; reopening replaces the operator's unfinished attempt,
  while other operators remain isolated by the single-flow lock
- invalid Telegram codes remain retryable, and the bucket browser now provides
  empty-bucket deletion, parent navigation, and skeleton loading on refresh and
  folder transitions
- the Connection tab can remove the current Telegram connection behind an
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

Multi-user boundary for the next increment:

- the current process still supports one active Telegram connection, but all
  new lifecycle work must preserve immutable connection/account ownership
  boundaries
- split bootstrap settings, session paths, storage-chat bindings, transport
  handles, and cleanup credentials by account/connection before allowing a
  second Telegram account to log in concurrently
- add two-account isolation, restart, and cross-account cleanup tests before
  changing the single-connection reservation into concurrent cleanup

Remaining Phase 10 work:

- release-grade deterministic fault/restart coverage now includes the persisted
  `recovery_required` + null-lease + `sending` regression, exact-byte mock
  reconciliation, corruption rejection, and a repeatable PowerShell release
  runner; the isolated live Telegram drill remains a separate RC/stable gate
- expand fault-injection coverage for Telegram timeouts, process crashes, and
  history-scan limits beyond the current deterministic matrix
- expand browser coverage from the Playwright authentication/navigation smoke
  tests to upload progress, retry/cancel, wizard, and degraded recovery flows
- add periodic reconciliation for staged uploads so missing chunks, orphaned
  staging trees, and interrupted commits are repaired or quarantined before
  they become user-visible corruption
- make the startup path fail soft for recoverable upload issues: the app should boot, expose health, and let operators inspect or repair state instead of disappearing when staging is damaged

Notes:

- `100 staged chunk(s) missing` is a durability symptom, not just a UI issue; the long-term fix is a worker-backed upload lifecycle plus reconciliation, not only a prettier error message.
- object and connection-removal cleanup is worker-backed: tombstone first, then
  let durable cleanup jobs remove Telegram payloads later, so deletes stay
  recoverable and retryable.
- the redesigned UI should keep health and recovery visible even when Telegram bootstrap is degraded, so operators can fix settings without losing the whole control plane.
