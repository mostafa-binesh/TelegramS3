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
- session reuse is persisted behind `TELEGRAM_SESSION_PATH`
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
- bounded range reads map manifest chunk spans back to the committed chunk files
- startup reconciliation repairs complete staged uploads, marks incomplete objects recovery-required, and quarantines orphaned data
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

- Status: pending
- Exit criteria:
  - an embedded high-performance frontend provides storage overview, endpoint details, capacity, and bootstrap status
  - the admin surface is protected by an authentication barrier before exposing operational data
  - the account setup and Telegram onboarding flow live behind the same authenticated app instead of a separate docker-compose service
  - the frontend can guide first-run setup for phone number, `.env` values, 2FA, and connection checks
  - no sensitive state is exposed before authentication succeeds

## Phase 9 - Multi-user control plane

- Status: pending
- Exit criteria:
  - multiple operator accounts are supported with explicit authorization boundaries
  - the authenticated frontend can distinguish per-user permissions and admin capabilities
  - onboarding, recovery, and maintenance actions are auditable by user
  - storage visibility and management actions remain consistent across concurrent users
  - tenant or workspace boundaries are documented if the model introduces them
