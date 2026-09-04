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
- Range reads must stay bounded in memory.
- Compatibility notes must distinguish implemented, designed, and unsupported behavior.

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
