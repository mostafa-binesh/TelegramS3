# Configuration

## Environment Variables

Use placeholders only:

```dotenv
TELEGRAM_API_ID=<your_api_id>
TELEGRAM_API_HASH=<your_api_hash>
TELEGRAM_PHONE_NUMBER=<optional_interactive_login_number>
TELEGRAM_SESSION_PATH=/var/lib/telegram-s3/session
TELEGRAM_STORAGE_CHAT_ID=<dedicated_private_channel_id>

TELEGRAM_METADATA_PATH=/var/lib/telegram-s3/metadata.sqlite
TELEGRAM_DATA_DIR=/var/lib/telegram-s3/data
TELEGRAM_S3_BIND_ADDR=127.0.0.1:9000
TELEGRAM_ADMIN_BIND_ADDR=127.0.0.1:9001

TELEGRAM_CHUNK_SIZE=1048576
TELEGRAM_CONNECTION_TIMEOUT_SECS=30
TELEGRAM_REQUEST_TIMEOUT_SECS=30
TELEGRAM_TRANSFER_TIMEOUT_SECS=900
TELEGRAM_RETRY_COUNT=5
TELEGRAM_RETRY_BACKOFF_MS=500
TELEGRAM_FLOOD_WAIT_RESPECT=true

TELEGRAM_PROXY_MODE=auto
TELEGRAM_PROXY_URL=socks5://127.0.0.1:1234
TELEGRAM_PROXY_USERNAME=
TELEGRAM_PROXY_PASSWORD=

TELEGRAM_S3_MASTER_KEY=<generate_secure_random_value>
RUSTFS_ACCESS_KEY=<generate_secure_random_value>
RUSTFS_SECRET_KEY=<generate_secure_random_value>
```

## Paths

- session path: persistent Telegram session database
- metadata path: local SQLite journal and indexes
- cache path: bounded manifest/chunk cache
- recovery path: exported backups and repair artifacts
- transport path: the Telegram session and proxy settings used by
  `auth login`, `auth status`, `auth logout`, `doctor`, and `server`
- object-format path: `TELEGRAM_DATA_DIR` now houses staged uploads,
  committed manifests, committed chunks, and quarantine artifacts during
  phase 3
- S3 bind address: `TELEGRAM_S3_BIND_ADDR` controls where the RustFS-backed
  `server` listener binds
- admin bind address: `TELEGRAM_ADMIN_BIND_ADDR` controls the loopback-only
  health and metrics listener

Defaults used by the current scaffold:

- metadata path: `data/metadata.sqlite`
- data dir: `data`
- S3 bind addr: `127.0.0.1:9000`
- admin bind addr: `127.0.0.1:9001`
- chunk size: `1 MiB`
- connection timeout: `30s`
- request timeout: `30s`
- transfer timeout: `900s`
- retry count: `5`
- retry backoff: `500ms`
- flood-wait respect: `true`
- proxy mode: `auto`

Proxy selection rules:

- `direct` mode disables proxy usage and rejects a proxy URL.
- `socks5` mode uses the provided SOCKS5 proxy URL directly.
- `auto` mode uses `socks5://` URLs directly and bridges `http://` or
  `https://` URLs through a local SOCKS5 listener.
- proxy credentials can be provided separately or embedded in the URL, but the
  resolved configuration must be internally consistent.

## Required Runtime Settings

- chunk size
- connection timeout
- request timeout
- transfer timeout
- retry count
- retry backoff
- flood-wait respect
- proxy mode
- local data directory

## Permission Checks

- session and key files must not be world-readable
- startup should reject obviously unsafe permissions when feasible

## Rotation

- rotate Telegram API credentials only with a fresh interactive login
- rotate S3 credentials independently of Telegram session material
- rotate encryption keys via versioned envelopes

## Validation Notes

- `doctor` validates required credentials, runtime settings, the SQLite
  metadata path, the object-format bootstrap state, the Telegram transport,
  the RustFS-backed S3 seam in live mode, and the loopback admin listener
  address.
- `server` performs the same bootstrap checks before binding the S3 listener
  and starting request processing. It also binds the loopback admin listener
  for `/healthz` and `/metrics`.
- Session and metadata paths are checked for unsafe `..` traversal, symlinks,
  and overly permissive permissions when the platform exposes those checks.
- `TELEGRAM_S3_MASTER_KEY` enables adapter-bound envelope encryption for chunk
  payloads and manifest encryption metadata. If it is missing, startup fails.
