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

- Status: pending
- Exit criteria:
  - chosen RustFS seam is implemented
  - bucket/object CRUD vertical slice works
  - range reads and listings are proven with a standard S3 client

## Phase 5 - Multipart and advanced compatibility

- Status: pending
- Exit criteria:
  - multipart recovery is durable
  - compatibility matrix is updated
  - crash and fault injection coverage exists

## Phase 6 - Production hardening

- Status: pending
- Exit criteria:
  - encryption, repair, garbage collection, metrics, and deployment docs are complete
  - recovery drills and benchmark plan are in place
