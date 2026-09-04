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
TELEGRAM_ADMIN_BOOTSTRAP_SECRET=<generate_secure_random_value>
TELEGRAM_ADMIN_UI_DIST_DIR=frontend/dist

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
  multipart scratch, quarantine artifacts, and mock-transport test blobs; the
  committed payloads themselves live as Telegram documents/messages
- S3 bind address: `TELEGRAM_S3_BIND_ADDR` controls where the RustFS-backed
  `server` listener binds
- admin bind address: `TELEGRAM_ADMIN_BIND_ADDR` controls the loopback-only
  health and metrics listener
- admin cookie secret: `TELEGRAM_ADMIN_BOOTSTRAP_SECRET` no longer acts as a
  login credential. When set it is used to derive the HMAC key that signs
  `/_admin` session cookies. Operator *identities* come from `metadata.sqlite`
  (`users`), not from the environment; see Operator accounts below.
- admin UI dist dir: `TELEGRAM_ADMIN_UI_DIST_DIR` points at the built Svelte
  assets served by the `/_admin` frontend path
- Docker deployments should mount `TELEGRAM_METADATA_PATH`,
  `TELEGRAM_DATA_DIR`, and `TELEGRAM_SESSION_PATH` on persistent volumes and
  set `TELEGRAM_S3_BIND_ADDR=0.0.0.0:9000` while leaving
  `TELEGRAM_ADMIN_BIND_ADDR=127.0.0.1:9001`; the admin frontend is served
  from the same Rust process on the reserved `/_admin` path. The data volume
  should grow with in-flight staging or quarantine, not with each successful
  backup, because committed object bytes are uploaded to Telegram.

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
- admin UI dist dir: `frontend/dist`

Proxy selection rules:

- `direct` mode disables proxy usage and rejects a proxy URL.
- `socks5` mode uses the provided SOCKS5 proxy URL directly.
- `auto` mode uses `socks5://` URLs directly and bridges `http://` or
  `https://` URLs through a local SOCKS5 listener.
- proxy credentials can be provided separately or embedded in the URL, but the
  resolved configuration must be internally consistent.

## Operator accounts

Passwords/accounts are stored in `metadata.sqlite` (schema `4`), hashed with
argon2id. There is no per-user `.env` entry.

- **First operator (server down):** `telegram-s3 users create <username> --password <pw>`
  seeds the superadmin. The first account is always forced to the `superadmin`
  role. Pass an empty password parameter via `TG_ADMIN_PASSWORD` env to avoid a
  shell-visible secret: `TG_ADMIN_PASSWORD=... telegram-s3 users create admin`.
- **After boot:** an authenticated superadmin can add/remove operators in the
  `/_admin` "Users" view, or use `telegram-s3 users list | status | password |
  delete`.
- Password changes revoke all of that user's sessions (`token_version` bump).
  There is **no email/password-reset flow**; recovery is CLI-admin only.
- Deleting the last remaining superadmin is refused.

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
  for `/healthz` and `/metrics`, while the authenticated operator frontend is
  served from the main listener under `/_admin`.
- The Docker image uses the same `server` path for foreground startup; the
  container entrypoint runs `config check` first and then starts the server in
  the foreground.
- Session and metadata paths are checked for unsafe `..` traversal, symlinks,
  and overly permissive permissions when the platform exposes those checks.
- `TELEGRAM_S3_MASTER_KEY` enables adapter-bound envelope encryption for chunk
  payloads and manifest encryption metadata. If it is missing, startup fails.
