# AGENTS.md

## Purpose

Telegram S3 exists to provide an S3-compatible object store whose durable data
plane is backed by Telegram documents and manifests. The project must remain
honest about Telegram limits, consistency, and recovery semantics.

## Non-goals

- Do not claim Telegram is a transactional object store.
- Do not rely on captions alone for critical metadata.
- Do not silently weaken security, redaction, or recovery guarantees.
- Do not hardcode real credentials in source, docs, tests, or examples.

## Repository Map

- `README.md` - project overview and status.
- `docs/upstream-analysis.md` - evidence gathered from upstream RustFS and Telegram Drive.
- `docs/s3-compatibility.md` - current compatibility matrix.
- `docs/telegram-storage-format.md` - manifest and chunk layout.
- `docs/configuration.md` - environment and runtime configuration.
- `docs/disaster-recovery.md` - recovery and rebuild procedures.
- `docs/adr/` - architecture decisions and rejected alternatives.
- `server_debug.md` - local-only live-server connection reference, read-only
  diagnostic commands, and the latest TelegramS3 production investigation.

## Build, Format, Lint, Test

When Rust code is present, use:

```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace --all-features
```

For security and dependency review, run:

```bash
cargo audit
cargo deny check
```

## UI Feature Coverage

- Every user-visible behavior change must add or update a Playwright test under
  `frontend/tests/`; `npm run check` and `npm run build` are compilation gates,
  not UI acceptance tests.
- UI tests must drive the running SPA in a real browser and assert the visible
  result, the relevant request payload, and the error/cancel path where one
  exists. Use mocked admin APIs for deterministic states, but do not describe a
  mocked API test as end-to-end UI coverage.
- File-management changes must cover the browser action path for upload,
  expiry, download, share, delete, folder navigation, and confirmation modals.
  Share tests must open the generated public URL in a separate browser context
  or page and verify the response/filename when the test owns a server fixture.
- Settings changes must cover tab navigation, initial values, validation,
  successful save, persistence/update feedback, and save failure. Destructive
  actions must cover cancel, confirmation, and the request sent after confirm.
- Before a push, run `npm run test:e2e` from `frontend` in addition to the
  Rust gates. A failing or missing UI scenario blocks release; leave the code
  unpushed and report the failure so it can be reviewed locally.
- New features are not complete until their UI scenario matrix is written down
  in the test file and the relevant happy, boundary, failure, and cancellation
  paths pass in the browser.

## Release and Docker Tags

- Docker publishing runs from version tags (`v*`) or manual workflow dispatch,
  not from every `main` push.
- Release candidates (`vX.Y.Z-rc.N`) must publish only explicit RC image tags.
  Do not move `latest`, `X.Y`, or `X` tags for prerelease builds.
- Stable version tags (`vX.Y.Z`) may update `latest`, `X.Y`, and `X`.
- When releasing, push the release commit and tag; expect GitHub Actions to run
  for the tag ref only.

## Secrets and Sessions

- Use environment variables for Telegram API credentials, S3 credentials, and encryption keys.
- Keep Telegram sessions on disk with restrictive permissions.
- Never log login codes, cloud-password values, session material, or encryption keys.
- Use dedicated interactive commands for login instead of prompting inside the server process.
- Use redacted logging for phone numbers, proxy credentials, and session paths.

## Storage Change Tests

Any storage change must include coverage for:

- manifest serialization and round-trips
- chunk mapping and range reads
- crash or restart recovery
- local metadata integrity and migrations
- compatibility behavior for the affected S3 operation

## Architectural Invariants

- Local metadata is authoritative for fast lookup, but Telegram manifests must be sufficient to rebuild the index.
- Partial uploads must not become visible as committed objects.
- Deletes must leave recoverable state before physical cleanup.
- A removal that requests remote deletion retains its owning Telegram transport
  until evidence and message cleanup complete; cleanup must never be routed
  through a replacement account. Local visibility is hidden immediately, but
  the owning connection remains reserved while `remote_cleanup_pending`.
- Range reads must stay bounded in memory.
- Every Telegram send must persist an attempt token and outcome before the
  remote call; ambiguous acknowledgements may be repaired only by matching the
  token and exact encrypted bytes, never by captions alone.
- Automatic retry is allowed only after a complete reconciliation scan proves
  that no matching remote document exists. Unbounded history scans, missing
  staging, unavailable Telegram, and byte mismatches remain recovery-required.
- Compatibility notes must distinguish implemented, designed, and unsupported behavior.

## Multi-user Preparation

- The current release has one active Telegram connection; do not treat that
  process-wide state as the long-term multi-user model.
- Future multi-user work must scope operator ownership, Telegram credentials,
  session paths, storage-chat bindings, connection generations, cleanup jobs,
  and authorization decisions by an immutable account/connection identifier.
- Never let a global `active_connection_id`, singleton transport, or shared
  session path decide ownership once more than one Telegram account is
  supported. A cleanup worker must use the credential/session snapshot owned by
  its job and must not fall back to whichever account logged in most recently.
- Multi-user changes must add isolation tests covering two accounts, account
  removal while another account is active, restart recovery, and cross-account
  cleanup rejection before enabling the feature.

## Documentation Updates

Whenever behavior changes, update:

- `README.md`
- `ROADMAP.md` so phase status, exit criteria, and completed work match the shipped behavior
- `docs/s3-compatibility.md`
- `docs/telegram-storage-format.md`
- `docs/disaster-recovery.md`
- the relevant ADR if the decision boundary changed

## Prohibited Shortcuts

- Do not buffer full objects in RAM unless explicitly documented and opt-in.
- Do not bypass local journals to "just write to Telegram".
- Do not hide unsupported S3 behavior behind a success response.
- Do not weaken permission checks or secret redaction to get green tests.

## Upstream Compatibility

- Preserve RustFS request semantics where possible.
- Keep Telegram Drive-derived ideas isolated behind backend modules and documented attribution.
- If a copied or adapted component is reused, retain its required notice.

## Definition of Done

The MVP is done only when a standard S3 client can create a bucket, put an object,
head it, get it, range-read it, list it, and delete it through a bounded,
recoverable Telegram-backed store with documented limitations and passing tests.

# Telegram connection

when you want to connect to Telegram from my pc, set socks5 proxy to localhost:12334
