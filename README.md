# Telegram S3

[![Docker image](https://github.com/mostafa-binesh/TelegramS3/actions/workflows/publish-docker-image.yml/badge.svg)](https://github.com/mostafa-binesh/TelegramS3/actions/workflows/publish-docker-image.yml)
[![License: Apache-2.0](https://img.shields.io/badge/license-Apache--2.0-blue.svg)](LICENSES.md)

An S3-compatible object storage server whose durable object format is designed
to be carried by private Telegram chats and channels — chunked, checksummed,
journaled, and encrypted at rest.

Telegram S3 speaks the S3 API to standard clients (`aws` CLI, SDKs, MinIO
tooling) through a RustFS-compatible server seam, while the storage engine
treats Telegram as a constrained remote object store rather than an unlimited
backup target. Because Telegram is not a transactional object store, local
metadata, operation journals, manifests, and recovery tooling are part of the
durability model — never an afterthought.

> **Status: in development (v0.4.0).** The full S3 surface, operator web UI,
> and durability machinery are implemented and tested. The Telegram transport
> layer (headless login, session reuse, proxy, retries) and an in-browser
> onboarding wizard are wired, but object bytes currently live on the local
> data directory; streaming them onto Telegram documents is the next milestone.
> See [Status](#status) below for the precise breakdown.

---

## Contents

- [Features](#features)
- [Status](#status)
- [Architecture](#architecture)
- [How storage works](#how-storage-works)
- [Quick start](#quick-start)
- [Configuration](#configuration)
- [S3 compatibility](#s3-compatibility)
- [Security and durability](#security-and-durability)
- [Operator web UI](#operator-web-ui)
- [Development](#development)
- [Documentation](#documentation)
- [License and acknowledgements](#license-and-acknowledgements)

## Features

- **S3-compatible API surface** — buckets, objects, byte-range reads, copy,
  conditional requests, versioned listings, delete markers, and full multipart
  upload sessions, served over a RustFS-backed listener.
- **Journaled, crash-safe object model** — uploads stage invisibly and commit
  atomically; interrupted writes never appear as objects; startup
  reconciliation repairs, rolls back, or quarantines incomplete state.
- **Chunked manifest format** — every object is a canonical manifest plus
  immutable, independently verifiable chunks (SHA-256 checked), designed to map
  one-to-one onto Telegram documents later.
- **Envelope encryption at rest** — chunk payloads are encrypted with
  ChaCha20-Poly1305 keyed from a local master key; range reads decrypt only the
  spans they touch, keeping memory bounded.
- **Telegram transport** — headless login, persisted session reuse, direct /
  SOCKS5 / bridged proxy support, explicit retry and flood-wait policy, and an
  in-browser onboarding wizard behind the operator UI.
- **Bounded memory everywhere** — uploads and downloads stream chunk-by-chunk;
  no whole-object RAM buffering (an explicit project invariant).
- **Operator web UI** — an authenticated `/_admin` Svelte app: dashboard,
  operator account management, bucket/object browser with per-file upload and
  ranged download, and the Telegram setup wizard.
- **Operational tooling** — a `telegram-s3` CLI (`users`, `config check`,
  `doctor`, `db`, `index`, `repair`, `gc --dry-run`), loopback-only health and
  metrics endpoints, and a production Docker image published to GHCR.
- **Multi-user control plane** — argon2id-hashed operator accounts in SQLite,
  HTTP-only session cookies, per-account login rate limiting and lockout,
  revocable sessions, and a superadmin role for account management.

## Status

The compatibility matrix in [docs/s3-compatibility.md](docs/s3-compatibility.md)
and the [ROADMAP](ROADMAP.md) are authoritative. In short:

| Area | State |
| --- | --- |
| S3 CRUD, range reads, copy, conditional requests | Implemented and smoke-tested against a standard S3 client |
| Multipart upload (initiate / part / complete / abort / list) | Implemented with durable local session state |
| Versioning, delete markers, lifecycle GC | Implemented as a compatibility layer over local metadata |
| Journaled object format + startup reconciliation | Implemented |
| Envelope encryption, repair, garbage collection | Implemented |
| Authenticated operator UI + onboarding wizard | Implemented |
| Multi-user accounts (Phase 9) | In progress; all accounts are admin-tier today |
| Real Telegram byte transport | **Next milestone** — bytes currently live under the local data dir with synthetic `local:` document IDs; manifests/chunks are laid out to move onto Telegram documents unchanged |
| Strong S3 transactional semantics from Telegram | Not a goal — see [Limitations](docs/limitations.md) |

Known constraints are documented honestly in
[docs/limitations.md](docs/limitations.md): Telegram files cap at ~2 GiB,
flood waits and rate limits are external, the local database is part of the
durability model, and the project must not be described as an unlimited or
sole backup target.

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
      |
      v
Private Telegram chats / channels   (future byte transport)
```

Two complementary surfaces serve the same store:

- the **S3 listener** (default `:9000`) is the public, protocol-compatible
  data plane;
- the **admin listener** (default loopback `:9001`) serves `/healthz` and
  `/metrics`, while the authenticated `/_admin` SPA and JSON API live on the
  public listener behind credentials.

A full request-lifecycle and consistency walkthrough is in
[ARCHITECTURE.md](ARCHITECTURE.md).

## How storage works

- One object = one canonical **manifest** (JSON: bucket, key, object ID,
  version, metadata, whole-object checksum, encryption state, chunk
  references) + one or more immutable **chunks** (~1 MiB each by default).
- Uploads write to staging, verify every chunk checksum, then publish the
  manifest and commit the local index **atomically** — readers never see a
  partial object.
- Deletes first record a recoverable tombstone, hide the object, then queue
  physical cleanup for conservative garbage collection.
- The manifest is the canonical recovery source: if local metadata is lost,
  the index can be rebuilt from manifests; if a journal entry points at
  nothing, reconciliation repairs or rolls it back.

See [docs/telegram-storage-format.md](docs/telegram-storage-format.md) for the
format details and [docs/disaster-recovery.md](docs/disaster-recovery.md) for
recovery procedures.

## Quick start

### Prerequisites

- Telegram `api_id` and `api_hash` from [my.telegram.org](https://my.telegram.org)
- A **dedicated** private Telegram channel or chat to act as the storage peer
  (id like `-100xxxxxxxxxx`)

### 1. Run with Docker

The published image is `ghcr.io/mostafa-binesh/telegrams3`
(built on every push to `main` and on `v*` tags; see
[.github/workflows/publish-docker-image.yml](.github/workflows/publish-docker-image.yml)).
Pinning a version tag such as `v0.4.0` is recommended for production.

```bash
export TELEGRAM_API_ID=123456          # from my.telegram.org
export TELEGRAM_API_HASH=0123...       # from my.telegram.org
export TELEGRAM_STORAGE_CHAT_ID=-100...   # your dedicated storage channel
export TELEGRAM_S3_MASTER_KEY=$(openssl rand -hex 32)   # envelope encryption
export RUSTFS_ACCESS_KEY=$(openssl rand -hex 16)        # S3 access key
export RUSTFS_SECRET_KEY=$(openssl rand -hex 32)        # S3 secret key
export TELEGRAM_ADMIN_BOOTSTRAP_SECRET=$(openssl rand -hex 32)  # signs /_admin session cookies

docker run -d --name telegram-s3 \
  --init --restart unless-stopped \
  -p 9000:9000 \
  -e TELEGRAM_API_ID -e TELEGRAM_API_HASH -e TELEGRAM_STORAGE_CHAT_ID \
  -e TELEGRAM_S3_MASTER_KEY -e RUSTFS_ACCESS_KEY -e RUSTFS_SECRET_KEY \
  -e TELEGRAM_ADMIN_BOOTSTRAP_SECRET \
  -e TELEGRAM_METADATA_PATH=/var/lib/telegram-s3/metadata/metadata.sqlite \
  -e TELEGRAM_DATA_DIR=/var/lib/telegram-s3/data \
  -e TELEGRAM_SESSION_PATH=/var/lib/telegram-s3/session/telegram.session \
  -v telegram-s3-metadata:/var/lib/telegram-s3/metadata \
  -v telegram-s3-data:/var/lib/telegram-s3/data \
  -v telegram-s3-session:/var/lib/telegram-s3/session \
  ghcr.io/mostafa-binesh/telegrams3:latest
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

Open <http://localhost:9000/_admin>, sign in with the account above, and follow
the in-browser **Telegram onboarding wizard** (phone → code → cloud password
when required) to authorize the storage peer. The readiness panel flips once
the session is live.

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

### 5. Build from source

```bash
cargo build --release
./target/release/telegram-s3 --help
```

## Configuration

Runtime configuration is environment-driven. The complete reference lives in
[docs/configuration.md](docs/configuration.md); the most important variables:

| Variable | Required | Purpose |
| --- | --- | --- |
| `TELEGRAM_API_ID`, `TELEGRAM_API_HASH` | yes | Telegram application credentials |
| `TELEGRAM_STORAGE_CHAT_ID` | yes | Dedicated storage channel/chat peer |
| `TELEGRAM_PHONE_NUMBER` | first login | Account to authorize (or use the UI wizard) |
| `TELEGRAM_S3_MASTER_KEY` | yes | Master key for envelope encryption |
| `TELEGRAM_ADMIN_BOOTSTRAP_SECRET` | yes | HMAC secret signing `/_admin` session cookies (not a login password) |
| `RUSTFS_ACCESS_KEY` / `RUSTFS_SECRET_KEY` | yes | S3 credentials clients must present |
| `TELEGRAM_S3_BIND_ADDR` | no | S3 listener address (default `127.0.0.1:9000`) |
| `TELEGRAM_ADMIN_BIND_ADDR` | no | Health/metrics listener, loopback only |
| `TELEGRAM_METADATA_PATH` / `TELEGRAM_DATA_DIR` / `TELEGRAM_SESSION_PATH` | no | Durable state locations |
| `TELEGRAM_PROXY_MODE` / `TELEGRAM_PROXY_URL` | no | Proxy transport (`direct`, `socks5`, `auto`...) |

Passwords and session material must come from the environment or the database —
never from source. Secrets are redacted from logs and diagnostics.

## S3 compatibility

Implemented operations include bucket create/delete/list/head, object
put/get/head/delete/list-v2/copy, byte-range GET, multipart initiate/upload/
complete/abort/list, conditional requests, versioning with delete markers, and
checksum enforcement. Presigned URLs, batch delete, bucket policies, object
tags, retention/object lock, event notifications, and quotas are documented
**gaps** — they are not silently emulated. The authoritative, per-operation
matrix is [docs/s3-compatibility.md](docs/s3-compatibility.md).

## Security and durability

- Chunk payloads are encrypted at rest (envelope, ChaCha20-Poly1305) under a
  locally held master key; the manifest records format and key fingerprint.
- Operator passwords are argon2id-hashed with per-account salts; sessions use
  HTTP-only cookies bound to a database row and are revocable; login is
  rate-limited with per-account lockout.
- Deletes leave recoverable state (tombstones) before any physical cleanup;
  garbage collection is conservative, retention-aware, and dry-run reviewed.
- Local metadata is a fast path, **not** the only source of truth — Telegram
  manifests are sufficient to rebuild the index.
- Secrets and sensitive paths are redacted from logs and CLI diagnostics.

See [SECURITY.md](SECURITY.md), [THREAT_MODEL.md](THREAT_MODEL.md), and
[docs/disaster-recovery.md](docs/disaster-recovery.md).

## Operator web UI

`/_admin` is an authenticated Svelte single-page app served by the Rust server
on the public listener. It provides a storage overview, endpoint and capacity
details, Telegram readiness, operator account management (superadmin-only),
and a bucket/object browser with per-file upload and full/range download —
streamed through the same bounded, checksum-verified chunk paths as the S3 data
plane. Guests see only the sign-in screen; every management and content API is
gated behind a user-bound session with CSRF protection.

## Development

```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace --all-features
```

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
