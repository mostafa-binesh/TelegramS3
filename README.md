# Telegram S3

[![Docker image](https://github.com/mostafa-binesh/TelegramS3/actions/workflows/publish-docker-image.yml/badge.svg)](https://github.com/mostafa-binesh/TelegramS3/actions/workflows/publish-docker-image.yml)
[![License: Apache-2.0](https://img.shields.io/badge/license-Apache--2.0-blue.svg)](LICENSES.md)

An S3-compatible object storage server. Buckets and objects are served to
standard S3 clients (`aws` CLI, SDKs, MinIO tooling) through a RustFS-compatible
listener, stored as a chunked, checksummed, journaled, and encrypted-at-rest
object format, and operated through an authenticated web UI.

The storage engine treats Telegram as a constrained remote object store rather
than an unlimited backup target. Because Telegram is not a transactional object
store, local metadata, operation journals, manifests, and recovery tooling are
part of the durability model - never an afterthought. Object bytes are stored
as Telegram documents/messages; local disk holds the control plane, transient
staging, and recovery artifacts, not committed payloads.

---

## Contents

- [Features](#features)
- [Architecture](#architecture)
- [How storage works](#how-storage-works)
- [Quick start](#quick-start)
- [Configuration](#configuration)
- [S3 compatibility](#s3-compatibility)
- [Security and durability](#security-and-durability)
- [Operator web UI](#operator-web-ui)
- [Roadmap](#roadmap)
- [Development](#development)
- [Documentation](#documentation)
- [License and acknowledgements](#license-and-acknowledgements)

## Features

- **S3-compatible API surface** — buckets, objects, byte-range reads, copy,
  conditional requests, legacy `ListObjects` plus `ListObjectsV2`, versioned
  listings, delete markers, and full multipart upload sessions, served over a
  RustFS-backed listener.
- **Journaled, crash-safe object model** — uploads stage invisibly and commit
  atomically; interrupted writes never appear as objects; background startup
  reconciliation repairs, rolls back, or quarantines incomplete state without
  delaying the listener while Telegram-backed recovery checks run.
- **Chunked manifest format** — every object is a canonical manifest plus
  immutable, independently verifiable chunks (SHA-256 checked) published as
  Telegram documents/messages.
- **Envelope encryption at rest** — chunk payloads are encrypted with
  ChaCha20-Poly1305 keyed from a local master key; range reads decrypt only the
  spans they touch, keeping memory bounded.
- **Telegram transport** — headless login, persisted session reuse, direct /
  SOCKS5 / bridged proxy support, explicit retry and flood-wait policy, and an
  in-browser onboarding wizard behind the operator UI. Login is considered
  ready only after the storage peer health probe succeeds; connection removal
  hides local data immediately but retains the owning generation and transport
  until requested remote cleanup completes, never routing old cleanup through a
  new account. Transient Telegram reads are retried per chunk using the same
  bounded policy before a download fails. A client stream remains open for up
  to 120 seconds while a transient Telegram/proxy read is recovering.
  Public-share and authenticated-admin range requests are additionally
  serialized per client IP and object, so segmented download tools cannot turn
  one object into several simultaneous Telegram reads; different objects remain
  independent.
- **Bounded memory everywhere** — uploads and downloads stream chunk-by-chunk;
  no whole-object RAM buffering (an explicit project invariant).
- **Download stage diagnostics** — the authenticated Overview keeps a bounded,
  process-local sample of recent public/admin/S3 reads, separating first-chunk,
  Telegram, retry-wait, decrypt, checksum-verify, and total timings for live
  performance testing, together with client-downloaded and Telegram-read byte
  totals. It also reports the adaptive prefetch window and the Telegram bytes,
  retries, chunks, and duration attributed to each account. A stream that
  delivers its final bytes successfully is recorded as completed even when the
  HTTP consumer closes without an extra terminal poll.
  It is diagnostic telemetry, not durable accounting.
- **Low-cost Overview polling** — storage counts and the logical committed
  Telegram payload size use a short-lived cached snapshot, while the
  five-second dashboard refresh uses a separate `/overview/live` payload for
  transfers, traffic, stage timings, and connection health. This prevents
  routine live telemetry from reparsing every manifest.
- **Verifier observability** — recovery scans emit start/end or failure logs
  with run duration and expose scan count, failure count, timestamps, and the
  last duration in the authenticated Overview. Cleanup telemetry separates
  due work, scheduled/not-yet-claimable work, and recovery-required targets.
  Storage policy also controls whether the first remote scan runs immediately
  when the server starts or waits for the configured interval, and provides a
  **Run integrity check now** action that resets the interval after the manual
  scan completes. It also provides **Run eligible cleanup now**, which wakes
  the durable cleanup worker without bypassing retention or recovery-required
  safeguards.
  When startup scanning is deferred, the Overview still shows the scheduled
  next run instead of waiting indefinitely for a first-scan timestamp.
- **Operator web UI** — an authenticated `/_admin` Svelte app: dashboard,
  operator account management, in-app bucket creation, bucket/object browser
  with per-file upload, ranged download, guarded folder deletion, and resilient
  folder transitions that keep the last listing visible while the next folder
  loads. The browser also supports server-paginated bucket/folder listings,
  bucket and recursive object search, result locations with direct parent-folder
   navigation, sortable bucket and object columns across paginated results, and
   compact icon-only per-row action menus that expose the applicable move, replicate,
   re-chunk, share, link-management, and delete operations alongside the fixed
   bulk-selection actions. Shared-link management uses a per-link expandable
   expiry editor so larger link sets remain usable on narrow screens. Each file
   also has an on-demand **Information** panel with its manifest, chunk layout,
   checksums, metadata, and replica locations. The path-
  style bucket names `_public` and `_admin` are reserved for
  the public-link and admin routes and are rejected by both the S3 and admin
  creation paths. The Telegram setup wizard is also available in the same
  console.
- **Operational tooling** — a `telegram-s3` CLI (`users`, `config check`,
  `doctor`, `db`, `index`, `repair`, `gc --dry-run`), loopback-only health and
  metrics endpoints, and a production Docker image published to GHCR.
- **Multi-user control plane** — argon2id-hashed operator accounts in SQLite,
  HTTP-only session cookies, per-account login rate limiting and lockout,
  revocable sessions, and a superadmin role for account management.
- **Bounded metadata connection pool** — the local SQLite store uses eight
  independently configured handles with WAL and a 30-second busy timeout;
  low-level handle failures are isolated and replaced without a schema change.
- **Account pool and replication** — multiple isolated Telegram connections can
  be registered, including the same Telegram account with separate session
  files. Each additional connection opens the same isolated four-step
  onboarding wizard as the primary connection, without overwriting primary
  draft values. Buckets support one-time or automatic physical replication or
  access-only sharing; replica/access badges and per-chunk account details are
  visible in Buckets, and ready replicas participate in round-robin reads. The
  Accounts workspace owns primary and additional connections together, with
  separate Connections, Replication, and Maintenance tabs; selected objects
  can be replicated from the same details flow as whole buckets. Each account
  has an independent persisted download eligibility switch, so an operator can
  temporarily remove one physical replica from read selection without changing
  ownership or cleanup. Overview health is aggregated across accounts and shows
  per-account connection indicators. Each new replica records its source chunk
  size; object and bucket account badges turn yellow when persisted replica
  layouts disagree with the canonical chunks. Downloads can retry a complete
  chunk on the selected account a configurable number of times, then fail over
  to the next enabled replica account. The integrity verifier checks sampled
  chunks through every recorded account, records durable per-account findings,
  and automatically re-uploads a damaged physical primary or replica from a
   verified alternate when one exists; recovered replica events remain visible
   in Recovery without hiding an otherwise healthy object. The Connections tab
   loads its account state once when opened and provides an explicit refresh
  action instead of polling the account list in the background. Every client
  read carries the persisted Telegram peer and message ID for its selected
  location, which is required when an access replica reads from a shared chat
  different from the selected account's configured storage chat.
- Automatic physical replication reuses existing encrypted chunk messages when
  the source and target connections use the same Telegram storage chat. This
  records a new account location without downloading or uploading the chunk;
  replication between different storage chats keeps the verified download /
  upload fallback.
- **Maintenance queues** — selected bucket objects can be re-chunked through a
  bounded durable worker. Objects are locked and report temporary
  unavailability while their replacement manifest is published. The bucket
  browser also supports selecting visible buckets for guarded bulk deletion or
  re-chunking every committed object in those buckets. Re-chunking does not
  automatically rebuild old physical replicas. The confirmation dialog can
  snapshot existing physical/access targets and queue durable follow-up jobs,
  or explicitly choose the faster primary-only path; primary-only means the
  replacement layout has no replica read paths until it is replicated again.
  The worker creates its receiving transfer before staging replacement chunks
  and reports the final partial chunk in durable progress.
- **Session recovery** — an expired admin session synchronizes with the
  guest-safe session endpoint and returns to the login screen instead of
  leaving a stale “not authenticated” error in the SPA.

## What's implemented

Everything described in [Features](#features) is live and smoke-tested against a
standard S3 client. Beyond the core CRUD path, that includes multipart sessions,
byte-range reads, conditional requests, version-aware listings and delete
markers, checksum enforcement, and retention-aware garbage collection.

Modern S3 semantics that can't be honored over the current store are not
silently emulated — known constraints are documented in
[docs/limitations.md](docs/limitations.md): Telegram files cap at ~2 GiB, flood
waits and rate limits are external, the local database is part of the durability
model, and the project is not an unlimited or sole backup target. Telegram is
treated as a constrained remote store, not a transactional object database; the
full per-operation matrix lives in
[docs/s3-compatibility.md](docs/s3-compatibility.md).

## Architecture

```text
S3 client / SDK
      |
      v
RustFS-compatible S3 server  (src/s3_server.rs)
      |
      v
Object-format service        (src/object_format.rs)
  manifest + chunk layout, journal, encryption, range reads
      |
      +-----> local SQLite metadata & journal  (src/metadata.rs)
      |
      v
Telegram transport           (src/telegram/)
  MTProto session, login, proxy, retry, flood-wait
```

Two complementary surfaces serve the same store:

- the **S3 listener** (default `:9000`) is the public, protocol-compatible
  data plane;
- the **admin listener** (default loopback `:9001`) serves `/healthz` and
  `/metrics`, while the authenticated `/_admin` SPA and JSON API live on the
  public listener behind credentials.

The S3 and admin listeners bind before the first full recovery snapshot. That
snapshot verifies committed Telegram chunks and is refreshed by the background
worker, so a slow or unavailable Telegram read does not leave a newly started
container with open Docker ports but no application listener.

A full request-lifecycle and consistency walkthrough is in
[ARCHITECTURE.md](ARCHITECTURE.md).

## How storage works

- One object = one canonical **manifest** (JSON: bucket, key, object ID,
  version, metadata, whole-object checksum, encryption state, chunk
  references, Telegram message/document IDs) + one or more immutable
  **chunks** (~1 MiB each by default).
- Uploads write to staging, verify every chunk checksum, publish the chunk
  payloads to Telegram, then commit the manifest and local index **atomically**
  — readers never see a partial object.
- Multipart completion composes the already-published part chunks into a schema
  v2 manifest and uploads only that final manifest. Per-chunk payload provenance
  preserves the original encryption identity, so a large completion does not
  download and re-upload the whole object.
- Every Telegram send has a durable attempt token and outcome. If a timeout or
  restart leaves the acknowledgement ambiguous, the worker scans recent
  Telegram documents for that token and repairs the checkpoint only after an
  exact encrypted-byte match; otherwise it schedules a safe retry. It never
  guesses from captions or silently duplicates an unknown send.
- Committed manifests and cleanup-evidence records are JSON documents on
  Telegram, but their remote filenames are intentionally extensionless opaque
  attempt tokens. The token is the reconciliation identity; it does not
  describe the document content.
- An explicit Telegram `FLOOD_WAIT` is different from a timeout: Telegram has
  rejected the send before publication, so the durable worker records a
  retryable attempt, waits for Telegram pacing, and resumes automatically.
  Historic flood-wait rows are upgraded only after any separate ambiguous
  acknowledgement has completed exact-byte reconciliation.
- Deletes first record a recoverable tombstone and hide the object, then queue
  evidence-first physical cleanup for the durable worker to remove Telegram
  messages asynchronously with retry support.
- The manifest plus its Telegram references are the canonical recovery source:
  if local metadata is lost, the index can be rebuilt from manifests; if a
  journal entry points at nothing, reconciliation repairs or rolls it back.

See [docs/telegram-storage-format.md](docs/telegram-storage-format.md) for the
format details and [docs/disaster-recovery.md](docs/disaster-recovery.md) for
recovery procedures.

## Quick start

### Prerequisites

- Telegram `api_id` and `api_hash` from [my.telegram.org](https://my.telegram.org)
- A **dedicated** private Telegram channel or group chat to act as the storage
  peer (id like `-100xxxxxxxxxx`; plain user ids will not work)

### 1. Run with Docker

The published image is `ghcr.io/mostafa-binesh/telegrams3`
(built on `v*` tags and manual workflow dispatch; see
[.github/workflows/publish-docker-image.yml](.github/workflows/publish-docker-image.yml)).
Pinning a version tag such as `v0.7.3-rc.4` is recommended when validating a
release. Release-candidate tags publish only their explicit RC tags;
the floating `latest`, major, and major-minor tags are reserved for stable
version tags without a prerelease suffix.

```bash
export TELEGRAM_S3_MASTER_KEY=$(openssl rand -hex 32)   # envelope encryption
export RUSTFS_ACCESS_KEY=$(openssl rand -hex 16)        # S3 access key
export RUSTFS_SECRET_KEY=$(openssl rand -hex 32)        # S3 secret key
export TELEGRAM_ADMIN_BOOTSTRAP_SECRET=$(openssl rand -hex 32)  # signs /_admin session cookies

docker run -d --name telegram-s3 \
  --init --restart unless-stopped \
  -p 9000:9000 \
  -e TELEGRAM_S3_MASTER_KEY -e RUSTFS_ACCESS_KEY -e RUSTFS_SECRET_KEY \
  -e TELEGRAM_ADMIN_BOOTSTRAP_SECRET \
  -e TELEGRAM_METADATA_PATH=/var/lib/telegram-s3/metadata/metadata.sqlite \
  -e TELEGRAM_DATA_DIR=/var/lib/telegram-s3/data \
  -v telegram-s3-metadata:/var/lib/telegram-s3/metadata \
  -v telegram-s3-data:/var/lib/telegram-s3/data \
  ghcr.io/mostafa-binesh/telegrams3:v0.7.1-rc.8
```

Or with the bundled [docker-compose.yml](docker-compose.yml) (local build):

```bash
cp .env.example .env        # fill in real values, never commit them
docker compose up -d --build
docker compose ps           # wait until healthy
```

### 2. Provision the first operator account

```bash
docker exec -it telegram-s3 telegram-s3 users create admin
```

The first account is forced to superadmin and can also be created while the
server is down. See [docs/configuration.md](docs/configuration.md).

### 3. Log in to the operator UI

Open <http://localhost:9000/_admin>, sign in with the account above, and use the
Telegram account view to enter or update the Telegram bootstrap values. The
image must contain the Vite-generated hashed files below
`TELEGRAM_ADMIN_UI_DIST_DIR/assets/`; a missing lazy-view file is reported with
a Retry action in the console.
Then use the in-browser **Telegram account wizard** (API credentials → storage chat →
network → phone/code/cloud password when required) to configure and authorize the single
Telegram session the store runs on. The inline wizard saves the required settings together
before sign-in, starts a fresh operator-owned flow each time it opens, and lets invalid codes
be retried without losing the active Telegram token. Operator
accounts in the "Operators" tab are separate from that Telegram login. The
overview now shows the Telegram storage connection state directly and flips to
connected once the session is authorized and the storage chat is reachable.

The account view also provides **Remove connection**. It shows the linked
Telegram account phone number and requires typing that exact number again as a
deliberate confirmation. The phone number is stored in local metadata so the
account identity can be shown back to the operator. Confirmation then hides the local
buckets, objects, and statistics immediately. An optional
checkbox queues deletion of the uploaded Telegram documents/messages in the
durable cleanup worker; while that queue is pending, the owning connection is
reserved so its cleanup cannot be sent through another account. Leaving it
clear disconnects locally while intentionally leaving those remote files in
Telegram. Recovery-required files, cancelled transfers, resumable sessions, and
their local staging records are also removed from the attention views for the
removed connection; tombstones and cleanup evidence remain until their safe
retention point. Bucket names are preserved exactly,
including Unicode, and ordinary bucket deletion remains empty-only and
tombstone-safe.

### 4. Use it as S3

Point any standard S3 client at the S3 listener:

```bash
export AWS_ACCESS_KEY_ID="$RUSTFS_ACCESS_KEY"
export AWS_SECRET_ACCESS_KEY="$RUSTFS_SECRET_KEY"
export AWS_DEFAULT_REGION=us-east-1
ENDPOINT=http://127.0.0.1:9000

aws --endpoint-url "$ENDPOINT" s3api create-bucket --bucket demo
aws --endpoint-url "$ENDPOINT" s3 cp ./hello.txt s3://demo/hello.txt
aws --endpoint-url "$ENDPOINT" s3 cp s3://demo/hello.txt ./downloaded.txt
aws --endpoint-url "$ENDPOINT" s3 ls s3://demo
```

Multipart, range requests, conditional requests, and version-aware listings
work through the same endpoint.

Public `/_public/<token>` and authenticated admin downloads advertise
`Accept-Ranges: bytes` and honor single byte-range requests, so a client can
resume after a disconnected stream instead of restarting the whole object. A
transient Telegram/network failure while fetching one chunk keeps the S3,
admin, or public response open while the server retries for up to 120 seconds;
missing messages, decryption failures, and checksum failures remain hard
recovery signals.
Public audio/video links also use `Content-Disposition: inline` with their
stored media type, allowing HTTP media players such as PotPlayer to probe and
play the URL; non-media public objects retain attachment disposition.

The Overview's **Download stage metrics** panel is a live testing aid for this
path. After a public, admin, or S3 read completes (or fails), it reports the
recent first-chunk, Telegram, retry-wait, decrypt, checksum-verify, and total
durations, along with chunk and payload counts. Samples are process-local and
bounded to the latest 20 reads, so this panel does not replace durable traffic
accounting or recovery records. Client-downloaded and Telegram-read byte sizes
are shown separately, and successful final-byte delivery is recorded as
completed even if the response consumer drops the stream immediately after
receiving those bytes. While a read is active, the panel also shows the object,
mode, current chunk, client-delivered bytes, Telegram-read bytes, retries, and
server/client chunk counts from process memory.

Per-object expiry is available as a Telegram S3 extension. Send either
`x-amz-meta-telegram-s3-expires-at: <RFC3339 timestamp>` or
`x-amz-meta-telegram-s3-expires-in: <positive seconds>` on a PUT (and on
multipart initiation). Expired objects disappear from S3/admin listings and
return as missing on reads; the background cleanup worker sweeps them into the
evidence-first tombstone path, after which the normal retention-aware GC policy
removes their Telegram data. Local tombstones and orphaned cleanup material
are retained for 12 hours by default before `gc` can remove them. The admin browser exposes the same seconds-
based expiry control. Operators can create bearer share links from the object
browser at `/_public/<token>`, with an optional link expiry capped by the object
expiry. Each file row shows how many active or expired-but-manageable links it
has; the link manager lists descriptions and URLs, copies a URL, changes its
expiry, and revokes it without leaving the bucket browser. Share-token hashes
are used for lookup and token ciphertext is encrypted in local metadata for
authenticated management; plaintext bearer tokens are not stored.

### 5. Build from source

```bash
cargo build --release
./target/release/telegram-s3 --help
```

## Configuration

Runtime configuration is mostly environment-driven, but Telegram bootstrap
settings, download prefetch, account-connection, and account-failover policies, upload chunk policy,
and recovery-verifier policy are managed from the authenticated admin panel and persisted in
`metadata.sqlite`.
`TELEGRAM_CHUNK_SIZE` is imported when no database policy exists; after that,
the database value is authoritative. The verifier defaults to one random chunk
per committed object every five minutes and can be changed or disabled live
from **Telegram settings → Storage policy**. The first remote scan runs at
startup by default and can be deferred until the interval from the same
setting. Download scheduling defaults to adaptive parallel mode. The alternate
sequential nearest-chunk mode fetches the first requested chunk first and then
preloads later chunks one at a time in manifest order. Both modes accept `0–4`
extra chunks; in sequential mode this is the maximum ordered look-ahead, while
adaptive mode uses it as a parallel ceiling. The maximum account-connection
policy accepts `1–5` accounts and is capped per file by the prefetch window and
the enabled replica accounts that have locations for that file. The reader
keeps at most one active Telegram payload read per account. Public and admin
range requests also allow only one active Telegram payload read per client IP
and object; sibling ranges wait rather than creating parallel backend work.
Live status stays in process memory rather than SQLite. Telegram
API IDs and storage chat IDs are validated as numeric values
before persistence; connection refresh failures are returned as JSON warnings
from the admin API rather than as proxy-level failures.
The account-failover setting defaults to one retry after the initial attempt and
accepts `0–8` retries before moving to another enabled replica location; each
attempt retains the normal per-chunk retry and 120-second stream recovery
behavior.
Cleanup retention is persisted in `app_settings` and can be changed from the
same page. It defaults to 12 hours, accepts one hour through 30 days, and
controls future delayed/orphan cleanup targets; existing scheduled and
recovery-required rows keep their current state. **Run eligible cleanup now**
only wakes the worker to process targets whose retention window has already
expired; it does not change any target's due time.
The complete reference lives in [docs/configuration.md](docs/configuration.md);
the most important variables:

| Variable | Required | Purpose |
| --- | --- | --- |
| `TELEGRAM_S3_MASTER_KEY` | yes | Master key for envelope encryption |
| `TELEGRAM_ADMIN_BOOTSTRAP_SECRET` | yes | HMAC secret signing `/_admin` session cookies (not a login password) |
| `RUSTFS_ACCESS_KEY` / `RUSTFS_SECRET_KEY` | yes | S3 credentials clients must present |
| `TELEGRAM_S3_BIND_ADDR` | no | S3 listener address (default `127.0.0.1:9000`) |
| `TELEGRAM_ADMIN_BIND_ADDR` | no | Health/metrics listener, loopback only |
| `TELEGRAM_METADATA_PATH` / `TELEGRAM_DATA_DIR` | no | Durable state locations (`TELEGRAM_DATA_DIR` is scratch, staging, and quarantine, not committed payload storage) |
| `TELEGRAM_RECOVERY_VERIFY_ENABLED` | no | Initial automatic verifier state when no database policy exists (default `true`; accepts `true`/`false`) |
| `TELEGRAM_RECOVERY_VERIFY_STARTUP` | no | Initial first-scan-on-startup policy when no database setting exists (default `true`; accepts `true`/`false`) |
| `TELEGRAM_RECOVERY_VERIFY_INTERVAL_SECS` | no | Initial sampled-verifier interval when no database policy exists (default `300`, minimum `60`) |
| `TELEGRAM_RECOVERY_VERIFY_CHUNKS` | no | Initial random chunks checked per committed object (default `1`, maximum `1024`) |

Passwords and session material must come from the environment or the database —
never from source. Secrets are redacted from logs and diagnostics.

## S3 compatibility

Implemented operations include bucket create/delete/list/head, object
put/get/head/delete/list-v1/list-v2/copy, byte-range GET, multipart
initiate/upload/complete/abort/list, conditional requests, versioning with
delete markers, and checksum enforcement. Presigned URLs, batch delete, bucket
policies, object tags, retention/object lock, event notifications, and quotas
are documented **gaps** — they are not silently emulated. Capability share
links are available through the admin panel, but AWS SigV4 presigned URLs
remain unsupported. The authoritative,
per-operation matrix is [docs/s3-compatibility.md](docs/s3-compatibility.md).

The bucket browser moves selected files and folders through a durable
manifest-only server operation. The browser and Telegram transport never
download or re-upload the payload: destination manifests reuse the existing
encrypted chunk references, are committed first, and source manifests are
tombstoned afterward. Folder moves preserve the selected folder name and
recursively move directory markers and descendants. A partial failure leaves
already committed destinations and untouched source data recoverable for
review.

## Security and durability

- Chunk payloads are encrypted at rest (envelope, ChaCha20-Poly1305) under a
  locally held master key; the manifest records format and key fingerprint.
- Operator passwords are argon2id-hashed with per-account salts; sessions use
  HTTP-only cookies bound to a database row and are revocable; login is
  rate-limited with per-account lockout.
- Deletes leave recoverable state (tombstones) before any physical cleanup;
  garbage collection is conservative, retention-aware, and dry-run reviewed.
- The admin bucket browser reports a delete as successful only after the local
  active pointer is removed. Non-empty folders are rejected; their child
  objects must be deleted first. Telegram payload removal remains asynchronous.
- Local metadata is a fast path, **not** the only source of truth — manifest
  documents are the authoritative recovery source, and the index can be rebuilt
  from them (see [docs/disaster-recovery.md](docs/disaster-recovery.md)).
- Secrets and sensitive paths are redacted from logs and CLI diagnostics.

See [SECURITY.md](SECURITY.md), [THREAT_MODEL.md](THREAT_MODEL.md), and
[docs/disaster-recovery.md](docs/disaster-recovery.md).

## Operator web UI

`/_admin` is an authenticated Svelte single-page app served by the Rust server
on the public listener. It provides a storage overview, endpoint and capacity
details, committed Telegram-file payload size, live transfer-pipeline analytics,
five-second network-traffic telemetry
with This session and all-time Total views split between clients and Telegram
payloads, system-readiness checks, Telegram
readiness, operator account management (superadmin-only),
in-app bucket creation and deletion (deletion remains empty-bucket-only), a routed
bucket/object browser with Unicode-preserving names, per-file upload, move, and full/range download -
streamed through the same bounded, checksum-verified chunk paths as the S3 data
plane. Each object row also includes a shared-link count and a polished link
manager for description, copy, expiry, and revoke actions. Guests see only the
sign-in screen; every management and content API is gated behind a user-bound
session with CSRF protection.
If a session cookie is rotated in another tab, the SPA automatically
resynchronizes its CSRF token and retries the rejected action once; genuine
authorization failures still remain visible to the operator.

The bucket browser also includes active S3 transfer jobs before their final
manifest is committed. A dedicated progress card shows completed/total parts,
percentage, and distinct receiving, uploading, finalizing, and needs-attention
states, plus a clear **Waiting for Telegram** state while a `FLOOD_WAIT` is
being paced automatically. At `269/269`, for example, the row says that the
final manifest is being published instead of implying that more parts remain.
Partial objects stay non-downloadable, and the browser retains its last
successful activity snapshot through a short activity-poll outage so an
in-flight row does not disappear and reappear. Folder navigation keeps the last
successful listing in place with a non-blocking loading indicator until the
next prefix response arrives.

Browser uploads use reception-only resumable sessions. A dropped connection can
continue from the server-reported chunk offset while the 120-second reception
lease remains active; the session is held in memory and is not resumable after a
server restart. Completed receptions enter the durable Telegram transfer queue.

## Roadmap

The near-term backlog focuses on durable upload/recovery hardening and final
deployment/browser acceptance:

- object durability hardening, including a durable upload worker / queue and
  stronger reconciliation for missing or orphaned staged chunks
- fault-injection, deployed-browser, and live Telegram verification of the
  operator workflows and recovery boundaries

The detailed version-by-version plan lives in [ROADMAP.md](ROADMAP.md).

## Development

```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace --all-features
```

The repeatable release checklist is available as
`./scripts/release-test.ps1`. It runs Rust, frontend, browser, packaging, and
available dependency checks. Add `-LiveTelegram` only for an isolated
pre-release Telegram drill; it requires dedicated `live-test` paths and the
local SOCKS5 proxy.

Smoke tests exercise the S3 CRUD/range path against a temporary server, the CLI,
the Docker packaging, session persistence, and the admin frontend plus
Telegram-wizard lifecycle. Security and dependency reviews:

```bash
cargo audit
cargo deny check
```

## Documentation

- [docs/configuration.md](docs/configuration.md) — environment and runtime configuration
- [docs/telegram-storage-format.md](docs/telegram-storage-format.md) — manifest and chunk layout
- [docs/telegram-transport.md](docs/telegram-transport.md) — login, session, proxy, retry behavior
- [docs/metadata-store.md](docs/metadata-store.md) — SQLite metadata schema and journals
- [docs/s3-compatibility.md](docs/s3-compatibility.md) — S3 compatibility matrix
- [docs/disaster-recovery.md](docs/disaster-recovery.md) — recovery and rebuild procedures
- [docs/limitations.md](docs/limitations.md) — honest limits and non-goals
- [docs/upstream-analysis.md](docs/upstream-analysis.md) — evidence from upstream RustFS and Telegram Drive
- [ARCHITECTURE.md](ARCHITECTURE.md), [ROADMAP.md](ROADMAP.md), [CHANGELOG.md](CHANGELOG.md)

## License and acknowledgements

Licensed under the Apache License 2.0 — see [LICENSES.md](LICENSES.md).

The S3 server seam builds on [RustFS](https://github.com/rustfs) (inspected at
`47a3f5ef0110ee5af04bbb761a8bb5ed99a9ce15`), and transport ideas draw on
Telegram Drive (inspected at `77518a93fbc8a8242f38e23e486a2d87d3f82fb2`).
Copied or adapted components retain their required notices.
