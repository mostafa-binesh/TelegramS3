# Phase 10 implementation handoff

Updated: 2026-09-08. Workspace: `D:\Programming\TelegramS3`. Branch: `main`.

Implementation note (2026-09-10): the lifecycle slice described in ADR-0008 is
now implemented locally. The metadata schema is v8, `TELEGRAM_SESSION_PATH`
is honored, removal detaches connection generations before remote cleanup, and
the wizard requires end-to-end Telegram health. This handoff remains useful
for the broader Phase 10 acceptance gaps; do not treat its older schema/version
references as the current runtime contract.

## Read this first

Phase 10 is still an uncommitted work in progress. It now formats, compiles, and
passes the current library unit tests, but it is not production-ready and has
not passed the Phase 10 acceptance matrix.

Do not reset the worktree. Before Phase 10 work began, only `README.md` and
`ROADMAP.md` were dirty (12 and 26 added lines). Those are the user's existing
edits and must be preserved. The remaining dirty and untracked source files are
the unfinished Phase 10 implementation.

No commit, tag, push, release, Docker deployment, live server start, or live
Telegram mutation was performed. Package versions remain `0.5.2`.

Read `AGENTS.md`, this file, and the actual diff before editing. Continue the
approved implementation without asking the user to approve the plan again. Do
not spawn agents unless the user or applicable instructions explicitly permit
delegation.

## First task in the next session: split `metadata.rs`

`src/metadata.rs` is currently 2,722 lines. Before adding more SQL or state
transitions, split it into cohesive modules as a behavior-preserving refactor.
Do not redesign the schema while moving code, and keep the existing public API
and serialized/database contracts stable.

Recommended layout:

```text
src/metadata.rs                 facade, public types/re-exports, MetadataStore
src/metadata/schema.rs          open pragmas, schema version, migrations
src/metadata/buckets.rs         bucket CRUD and bucket invariants
src/metadata/manifests.rs       journal, manifests, active index, tombstones
src/metadata/multipart.rs       multipart sessions and part persistence
src/metadata/auth.rs            users, sessions, setup-complete persistence
src/metadata/settings.rs        Telegram bootstrap settings
src/metadata/recovery.rs        rebuild, verify, recovery markers and reports
src/metadata/rows.rs            shared row decoders and timestamp helpers
```

`src/durable.rs` already contains Phase 10 transfer/cleanup state. After the
metadata split is green, consider splitting it into `durable/jobs.rs`,
`durable/cleanup.rs`, and `durable/models.rs`; do not combine that second
refactor with correctness changes.

Refactor rules:

1. Move one concern at a time and run `cargo fmt --all -- --check` plus
   `cargo test --lib` after each move.
2. Keep `MetadataStore`, `MetadataError`, and existing public record types at
   their current import paths through re-exports.
3. Let child modules add separate `impl MetadataStore` blocks. Keep shared SQL
   helpers `pub(super)` or `pub(crate)` only where required.
4. Move tests beside their owning module. Retain explicit migration and
   cross-module transaction tests in `schema.rs` or a dedicated integration
   test.
5. Review the diff after the split to ensure it is mechanical before resuming
   Phase 10 logic.

## Approved product decisions

- Use one SQLite-backed background workflow inside the existing Rust server
  and container. Do not add Redis, a broker, or another Compose service.
- Cover PUT, browser upload, directory markers, copy, multipart parts and
  completion, DELETE, multipart abort, and obsolete-part cleanup.
- Browser upload returns `202` only after complete durable local staging and
  returns a durable job ID. S3 write success still means final object/part
  commit, not merely queued data.
- Incomplete request reception requires client resubmission. A fully staged
  upload must survive disconnect, timeout, process crash, and restart.
- Public first-superadmin setup is shown in the normal web UI whenever setup
  has never completed and no operator exists. There is no setup token and no
  loopback-only restriction. Successful setup must atomically and permanently
  close public setup. Preserve CLI account provisioning.
- UI direction remains an operations console with Overview, Buckets,
  Transfers, Recovery, Telegram Settings, Operators, and a persistent health
  badge based on a real Telegram check timestamp.
- Browser resumable multipart negotiation, nested directory upload, and ZIP
  downloads remain deferred.
- Final acceptance requires automated crash/fault/API/browser/Docker tests and
  a real isolated Telegram drill. From this PC, use
  `socks5://localhost:12334` for Telegram.

## What is implemented so far

### Durable transfer reception and jobs

- Schema version is 6 and draft tables cover transfer jobs, chunk checkpoints,
  multipart job mappings, cleanup targets, send-attempt evidence, and persisted
  setup completion.
- `TELEGRAM_STAGING_MAX_BYTES` was added with a validated 10 GiB default.
- Browser/raw-body upload creates the receiving job before reading the body,
  rechunks by configured chunk size, encrypts chunks, flushes files, reserves a
  global staging budget, writes the staged manifest, queues the job, and then
  returns `202` with its job ID.
- Slow asynchronous reception has a 30-second receiving-lease heartbeat.
- Failed/incomplete reception now attempts to delete partial local files,
  releases reserved bytes when deletion succeeds, removes partial metadata,
  records a terminal reception failure, and tells the client to resubmit.
- The synchronous staging path has a drop guard that performs equivalent
  cleanup if staging fails before durable acceptance.
- Transfer list/detail/retry/cancel endpoints and frontend transfer polling are
  present. Job fields include byte/chunk progress, attempts, retry deadline,
  timestamps, and redacted operator-facing failures.

### Publication, ordering, and ambiguity

- Two chunk uploads can run concurrently within a claimed job.
- Every Telegram send now creates a durable `transfer_send_attempts` row before
  sending and atomically checkpoints the returned location afterward.
- An expired send attempt with no checkpoint is treated as an unknown Telegram
  acknowledgement and moved to recovery instead of being silently replayed.
- Transfer commit has a strict transactional entry point that validates job
  lease, lease expiry, operation identity, and per-key order while atomically
  updating manifest state, active index, journal, job completion, and multipart
  completion state.
- Claiming blocks a newer same-key job behind an older nonterminal job. Draft
  data where a newer write already completed defensively marks the older job
  superseded.
- The current restart test now exercises a persisted queued upload by dropping
  and reopening the service before waiting for completion.

### Delete and cleanup work

- Non-versioned key deletion now cancels queued/receiving work even when no
  active object exists. Version-targeted deletion no longer cancels unrelated
  pending writes for the same key.
- Tombstones enqueue an evidence target plus individual Telegram message
  targets. Message deletion cannot be claimed until deletion evidence has been
  published and checkpointed.
- Cleanup claims, leases, retry state, per-target completion, already-missing
  mock messages, and legacy tombstone backfill exist.
- The background cleanup loop now drains the cleanup outbox instead of calling
  the old direct Telegram garbage collector. Manual garbage collection makes
  eligible outbox rows due and uses the same worker machinery.
- Unknown acknowledgement while publishing deletion evidence is conservatively
  quarantined and retains recovery material.
- Read streams now hold in-process object pins, and cleanup checks committed
  manifests and multipart parts before deleting a shared Telegram reference.
- Mock Telegram message IDs now use a SQLite-serialized counter, seeded above
  existing mock files, so concurrent chunk uploads cannot choose the same ID.

### Multipart work

- Multipart parts flow through the common bounded encrypted chunk writer.
- A part stores an embedded manifest with a fresh object/encryption identity.
- Replacing a part queues the old part references for cleanup.
- Private completed part manifests are removed from the ordinary manifest,
  journal, and recovery-marker tables; `wait_transfer` resolves their embedded
  part manifest instead.
- Completion streams selected part manifests through the common writer and
  commits the final object and session transition in one SQLite transaction.
- Completed-session retry can return the already-visible final object.
- Abort rejects actively uploading/committing part jobs and schedules known
  completed part references for cleanup.
- Legacy multipart rows without an embedded manifest remain explicitly
  recovery/re-upload only because the old framing can be undecodable.

### Authentication and first-account setup

- Guest-safe setup status and same-origin first-account POST routes exist.
- Browser first-account creation is atomic and forces `superadmin`.
- CLI/general `create_user` now also atomically forces the first account to
  `superadmin` and persists `setup_complete=true`; deleting users cannot reopen
  public setup.
- Metadata invariants prevent deleting or disabling the last enabled
  superadmin. CLI and admin prechecks count enabled superadmins rather than all
  users.

### Runtime health, lifecycle, and metrics

- S3 and admin listeners bind before Telegram reconciliation/connection work.
- `/healthz` remains a fast local liveness endpoint and `/readyz` now considers
  cached Telegram state/check age, staging capacity, and recovery state.
- Telegram manager initialization is lazy. The health loop has singleton
  guarding, a 10-second probe timeout, a real `help.getConfig` RPC for non-mock
  connections, check timestamps, and last-success state.
- The known transport read-lock/write-lock risk was removed by cloning the
  current transport outside the read-guard scope.
- Object transfer/cleanup workers and the Telegram health monitor now have
  shutdown signals and join handles; server shutdown awaits them.
- S3 requests use the configured transfer timeout for PUT/POST/DELETE and the
  configured request timeout for reads instead of applying the transfer timeout
  to every operation.
- Metrics now expose pending jobs, oldest pending age, retries, failures,
  staging bytes, cleanup backlog/recovery, and Telegram check time.
- SQLite file stores now set a busy timeout, WAL journal mode, `FULL`
  synchronous durability, and foreign keys. Draft schema repair adds newly
  introduced v6 columns/tables if a local v6 database was opened by an earlier
  Phase 10 run.

### Frontend work

- New components: `HealthBadge.svelte`, `Sidebar.svelte`,
  `SetupWizard.svelte`, and `Transfers.svelte`.
- `App.svelte` dispatches setup/login/console, presents six console views, and
  retains the existing bucket, operator, and Telegram settings functionality.
- Browser upload uses the asynchronous endpoint, reports local staging
  separately from final Telegram commit, and directs accepted uploads to
  Transfers without resending them.
- The visual system has a compact neutral operations-console palette, focus
  styles, and less decorative rounding.

## Verification completed in the latest session

The following are current as of this handoff:

```powershell
cargo fmt --all
cargo check --workspace --all-features --message-format short -j 2
cargo test --lib -- --nocapture
# 47 passed, 0 failed

cd frontend
npm run check
# 0 errors, 0 warnings
```

This proves formatting/compilation and the existing library unit suite only.
It does not prove crash safety, outbox correctness, browser behavior, Docker
behavior, bounded memory, or live Telegram behavior.

## What is not finished or not verified

### Highest-priority backend correctness

1. Persist and recheck actual S3 write/delete conditionals at publication.
   Per-key sequencing reduces races but does not make two conditionally accepted
   requests correct if both passed handler-time checks.
2. Add deterministic fault points and subprocess crash tests for reception,
   every send/checkpoint boundary, remote manifest publication, local commit,
   cleanup evidence, each message deletion, and local staging cleanup.
3. Review send-attempt recovery UX. Unknown acknowledgements are retained
   conservatively, but there is no operator reconciliation action that can
   confirm/quarantine a remote duplicate and safely resume.
4. Thoroughly test the new cleanup outbox. Its schema, claim logic, evidence
   publication, shared-reference scan, retention, migration backfill, retry,
   and local removal have only compiled and passed the old zero-retention GC
   unit test.
5. Store original tombstone due times during legacy backfill. Current backfill
   conservatively starts a fresh seven-day retention window.
6. Add durable/read-safe handling beyond in-process pins where appropriate and
   test copy/download versus retention-expiry cleanup races.
7. Verify receiver accounting on disk-full, rename failure, Windows replacement
   behavior, zero-byte objects, checksum mismatch, cancel, and cleanup failure.
8. Review fairness and concurrency. There is one claimed job worker with
   two-way chunk fan-out, not two independently claimed job workers.
9. Add bounded persisted reconciliation cursors, orphan scans, expired receiver
   handling, interrupted-commit recovery, and non-starving local cleanup.
10. `recovery_snapshot`/`cached_recovery_issues` scaffolding exists but is not
    populated or used. The admin overview still awaits `recovery_issues()` and
    can perform remote work on the request path. Move this to a bounded
    background snapshot.
11. Replace the placeholder `POST /_admin/api/recovery/repair` response with a
    real scoped action using the common reconciliation machinery, or return a
    clear unsupported response until it exists.
12. Review managed task shutdown under a hung transport, panic, repeated
    start/stop, and runtime restart. It currently has compile/unit coverage only.

### Multipart and S3 compatibility

- Test concurrent part replacement, completion versus abort, duplicate
  completion, cleanup ownership, restart at every boundary, and closed-session
  behavior.
- Review all `UploadPartCopy` and copy ranges for whole-range buffering and use
  the bounded common reader everywhere.
- Add standard-client coverage for CRUD, head/list/range/empty/boundaries,
  conditionals, version listing/delete, copy, multipart, upload-part-copy,
  abort, timeout beyond 60 seconds, restart, and memory bounds.
- Verify directory markers and all synchronous browser/S3 compatibility paths
  use the durable workflow without changing response semantics.

### Health, readiness, metrics, and auth

- Preserve `last_success_at` across later failed probes and verify that the
  chosen RPC proves current connectivity without leaking details.
- Populate the UI badge from the actual Telegram check timestamp and expose a
  useful explanation for stale, disconnected, reauth, and not-configured states.
- Decide and test exact readiness policy for queue backlog and recoverable
  per-object failures; current policy is new and unreviewed.
- Add tests for metrics values and ensure labels/errors never expose secrets.
- Test setup Host/port/origin handling, missing-Origin policy, rate limiting,
  concurrent first-account requests, CSRF, roles, session revocation, last
  superadmin rules, and credential redaction. The current setup limiter records
  a failure before validation and needs review/reset behavior on success.

### Frontend, browser, Docker, security, and release

- Functionally and visually inspect desktop/mobile layouts. Existing large
  blocks remain in `App.svelte`; extract reusable notifications, forms, and
  tables where it improves behavior and testing.
- Fix transfer/recovery filtering before pagination, action availability,
  encrypted-versus-plain byte labels, stale/loading/error states, session
  expiry, polling backoff, and accessibility details.
- Add a browser test harness with a real backend fixture and cover setup,
  login/logout, guest gating, roles, accepted-versus-committed upload, reload
  progress, retry/cancel, degraded settings/recovery, and responsive layouts.
- Add `TELEGRAM_STAGING_MAX_BYTES` to `.env.example`, Compose, configuration
  docs, and locked Docker tests.
- Add general CI plus dependency/security review. `cargo-audit` and
  `cargo-deny` are not installed; `deny.toml` does not exist.
- Run the full workspace test suite, strict Clippy, frontend build, Docker build,
  Compose validation, preserved-volume recreation, unhealthy-Telegram control
  plane test, and shutdown/restart smoke tests.
- Run the isolated real Telegram interruption/restart/range/hash/tombstone drill
  through `socks5://localhost:12334`. Do not read or mutate the existing ignored
  `data/` state for this drill; use isolated metadata, session, and object names.
- Update `README.md`, `ROADMAP.md`, S3 compatibility, storage format, disaster
  recovery, configuration, limitations, and the relevant ADR only after the
  behavior is settled. Preserve the user's existing README/ROADMAP edits.
- Set version `0.6.0` consistently in Rust and frontend lockfiles only after all
  acceptance gates pass. Then make a scoped release commit/tag/push using the
  tag-only Docker publishing rules. Do not release based on current tests.

## Recommended execution order

1. Perform the behavior-preserving `metadata.rs` module split and keep all 47
   library tests green.
2. Add focused metadata/durable tests for v5-to-v6 migration, draft-v6 repair,
   setup races, claim fencing, per-key order, delete-with-pending-work, send
   ambiguity, cleanup dependencies, and mock ID concurrency.
3. Finish condition persistence, recovery actions, cleanup/outbox semantics,
   multipart atomicity, read/copy protection, and bounded reconciliation.
4. Finish task lifecycle, cached recovery/health snapshots, readiness, and
   metrics tests.
5. Finish frontend integration and browser tests, then visually inspect it.
6. Add CI/security/Docker configuration and run all local gates.
7. Run the isolated live Telegram drill only when the code and harness are
   ready and necessary credentials are available without printing secrets.
8. Update documentation/version and release only after every gate is recorded
   as passed.

## Required final gates

```powershell
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace --all-features
cargo audit
cargo deny check

cd frontend
npm run check
npm run build
# Add and run the browser test command.
```

Also require the crash/fault matrix, standard S3 client suite, memory-bound
tests, locked Docker/Compose tests, and isolated live Telegram drill described
above. A green build or unit suite is not Phase 10 completion.

## Environment notes

- Windows PowerShell. `python.exe` resolves to WindowsApps; if a script is
  unavoidable use `py -3 -X utf8`.
- Cargo, Node/npm, and Docker are available. Run npm commands from `frontend/`.
- `StreamingBlob::wrap` requires `Send + Sync`; shared Telegram reader futures
  are only `Send`. The working pattern is
  `Body::http_body_unsync(StreamBody::new(stream.map(...Frame::data))).into()`.
- Pinned `s3s` sources are in the Cargo checkout. Inspect the pinned API before
  changing request/response interfaces.
- No `.env` exists. Existing ignored `data/` contains metadata/session/storage
  state whose secrets/content were not read or used.
- No long-running command or server was left running by this session.

## Prompt for a fresh session

> Continue Phase 10 in `D:\Programming\TelegramS3`. Read `AGENTS.md` and
> `docs/phase10-handoff.md`, inspect the dirty worktree, and preserve my existing
> README/ROADMAP edits and all unfinished source. First split the 2,722-line
> `src/metadata.rs` into cohesive modules as a behavior-preserving refactor,
> keeping public paths and tests stable. Then continue the listed correctness
> and acceptance work. Do not re-plan, reset, commit, tag, push, release, or
> mutate live Telegram data merely because the code compiles. Public first-user
> setup has no token. For isolated live Telegram testing from this PC use
> `socks5://localhost:12334`, never expose secrets, and never use the existing
> ignored `data/` state for destructive tests.
