# Configuration

## Environment Variables

Use placeholders only:

```dotenv
TELEGRAM_METADATA_PATH=/var/lib/telegram-s3/metadata.sqlite
TELEGRAM_DATA_DIR=/var/lib/telegram-s3/data
TELEGRAM_SESSION_PATH=/var/lib/telegram-s3/session/telegram.session
TELEGRAM_S3_BIND_ADDR=127.0.0.1:9000
TELEGRAM_ADMIN_BIND_ADDR=127.0.0.1:9001
TELEGRAM_ADMIN_BOOTSTRAP_SECRET=<generate_secure_random_value>
TELEGRAM_ADMIN_UI_DIST_DIR=frontend/dist

TELEGRAM_CHUNK_SIZE=1048576
  TELEGRAM_RECOVERY_VERIFY_ENABLED=true
  TELEGRAM_RECOVERY_VERIFY_STARTUP=true
  TELEGRAM_RECOVERY_VERIFY_INTERVAL_SECS=300
TELEGRAM_RECOVERY_VERIFY_CHUNKS=1
TELEGRAM_CONNECTION_TIMEOUT_SECS=30
TELEGRAM_REQUEST_TIMEOUT_SECS=30
TELEGRAM_TRANSFER_TIMEOUT_SECS=900
TELEGRAM_RETRY_COUNT=5
TELEGRAM_RETRY_BACKOFF_MS=500
TELEGRAM_FLOOD_WAIT_RESPECT=true

TELEGRAM_S3_MASTER_KEY=<generate_secure_random_value>
RUSTFS_ACCESS_KEY=<generate_secure_random_value>
RUSTFS_SECRET_KEY=<generate_secure_random_value>
```

## Paths

- session path: persistent Telegram session database. Set
  `TELEGRAM_SESSION_PATH` explicitly in deployments; when it is unset the path
  derives from the metadata path as `<metadata-dir>/telegram.session`. The
  resolved path is used consistently by login, status, doctor, and the server.
- metadata path: local SQLite journal and indexes
- cache path: bounded manifest/chunk cache
- recovery path: exported backups and repair artifacts
- transport path: the resolved Telegram bootstrap settings persisted in
  `metadata.sqlite` and used by `auth login`, `auth status`, `auth logout`,
  `doctor`, and `server`; the session file path itself is system-owned and must
  be mounted persistently in Docker.
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
  assets served by the `/_admin` frontend path. Vite's `index.html` and its
  hashed lazy-view chunks must be present under this directory's `assets/`
  subdirectory; if a lazy chunk cannot be loaded, the console shows a retry
  action instead of remaining on an infinite skeleton.
- Docker deployments should mount `TELEGRAM_METADATA_PATH` and
  `TELEGRAM_DATA_DIR` on persistent volumes and set
  `TELEGRAM_S3_BIND_ADDR=0.0.0.0:9000` while leaving
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
- download prefetch: `1` extra verified chunk (`0–4`)
- account failover: `1` retry after the initial complete chunk attempt (`0–8`)
- recovery verifier: enabled
- recovery verifier interval: `300s` (5 minutes)
- recovery verifier sample: `1` random chunk per committed object
- connection timeout: `30s`
- request timeout: `30s`
- transfer timeout: `900s`
- retry count: `5`
- retry backoff: `500ms`
- flood-wait respect: `true`
- admin UI dist dir: `frontend/dist`

Telegram-backed reads use `retry count`, `retry backoff`, and flood-wait
settings for transient chunk-download failures. Each retry re-fetches the
current complete chunk before it is emitted, so partial or unverified data is
never forwarded. The reader may also prefetch the configured number of extra
chunks in parallel, but preserves output order and never emits a prefetched
chunk before its decryption and checksum verification complete. Public share
responses advertise `Accept-Ranges: bytes`; a client can resume an exhausted
stream with a single byte range without any additional setting.

The prefetch scheduler starts with a small window, ramps up after clean reads,
and backs off after retries or a measured throughput drop. It permits only one
active Telegram payload read per account, so a single account is not made to
compete with itself; enabled physical replicas can still provide parallel
capacity. The Overview Download stage metrics show the final/max window and
per-account chunk, byte, retry, and Telegram-duration totals.
For public links, stored `audio/*` and `video/*` objects use
`Content-Disposition: inline` so media players can open the URL directly;
other content types continue to use attachment disposition.

## Operator accounts

Passwords/accounts are stored in `metadata.sqlite` (schema `5`), hashed with
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

The authenticated browser keeps the CSRF header synchronized with the
HTTP-only session cookie. If another tab rotates the session and an action
receives `invalid csrf token`, the SPA reads the current session and retries
that action once without requiring a page reload. This does not change the
server-side CSRF requirement or session expiry behavior.

## Dynamic Storage Policy

`TELEGRAM_CHUNK_SIZE` is used as the one-time import value when an existing
metadata database has no stored chunk policy. The value is then persisted in
the `app_settings` table and becomes authoritative. The authenticated admin
console exposes it under **Telegram settings → Storage policy** and applies a
new value immediately to new uploads and resumable receptions. Existing
manifests and active transfers retain their recorded chunk boundaries; no
rechunking or Telegram migration is performed.

The same page controls `telegram_cleanup_retention_secs`. Its database default
is `43200` seconds (12 hours), with an allowed range of one hour through 30
days. It applies to future delayed/orphan cleanup targets and does not rewrite
existing scheduled or `recovery_required` cleanup rows.

The same page controls `telegram_download_prefetch_chunks`, the number of
extra complete chunks allowed ahead of a client download. Its database default
is `1`; `0` disables look-ahead and `4` is the maximum. The setting was
introduced by metadata schema v13 and is safe to change without rewriting
existing objects or changing active transfer chunk boundaries. In adaptive
parallel mode it is a concurrency ceiling; the scheduler may use a smaller
window when an account is retrying or slowing. In sequential nearest-chunk mode
it is an ordered look-ahead queue: the first chunk is read first, then later
chunks are fetched one at a time, never concurrently, until the queue is full.

The `telegram_download_prefetch_mode` setting was introduced by schema v23 and
defaults to `adaptive`. `sequential` is useful when one Telegram account has a
shared bandwidth limit and predictable nearest-chunk reads are preferable to
parallel pressure. Existing objects and their chunk layout are unchanged.
Live active-download status is kept in process memory and is not persisted as
per-chunk SQLite state.

The `telegram_download_account_connections` setting was introduced by schema
v24 and defaults to `5`, with an allowed range of `1–5`. In adaptive mode it
limits the number of Telegram chunk reads that one client download may have
active at once. The effective value is automatically reduced to the smaller of
the configured limit, the prefetch window, and the enabled replica/access
accounts that have a location for the object. Sequential mode still performs
one remote read at a time. Each account retains its separate one-read limiter.

The same page controls `telegram_download_failover_retries`, introduced by
schema v19. It is the number of additional complete attempts made on the
currently selected eligible Telegram account before the reader rotates to the
next eligible replica/access location. `0` moves after the first failed
attempt; `1` allows two total attempts. Each attempt still uses the normal
transport retry policy and 120-second stream recovery window, and each chunk is
decrypted and verified as one unit rather than assembled from multiple
accounts. The value changes live and persists across restart.

The same policy page controls the sampled recovery verifier. When enabled, it
runs once at startup by default and then at the configured interval, selecting the
requested number of distinct chunk indexes uniformly at random for each healthy
committed object. The verifier can be disabled completely; while disabled it
does not run automatic remote checks or quarantine objects from verifier scans,
and the existing interval/sample values remain saved for the next enablement.
The sample is without replacement within one scan, but coverage across scans is
probabilistic. `TELEGRAM_RECOVERY_VERIFY_ENABLED`,
`TELEGRAM_RECOVERY_VERIFY_STARTUP`,
`TELEGRAM_RECOVERY_VERIFY_INTERVAL_SECS`, and
`TELEGRAM_RECOVERY_VERIFY_CHUNKS` are one-time import defaults for databases
that have no stored policy; the database values are authoritative afterward.
The overview shows the enabled/disabled state, next scheduled scan, sample
size, distinct confirmed broken-file count, and the current verifier problem
list.
The Storage policy page also has **Run integrity check now**. It queues the
same verifier worker path immediately; after that scan finishes, the normal
interval starts over, so a manual run does not cause an additional near-term
scheduled scan. The action is disabled when automatic verification is disabled.

The same page has **Run eligible cleanup now**. It wakes the existing durable
cleanup worker immediately, but the worker still claims only targets whose
retention window has expired. It does not rewrite `due_at`, bypass
`recovery_required`, skip evidence-first ordering, or delete messages that are
still referenced or being read. This is a worker wake action and requires no
database migration.

The authenticated Overview also shows `Telegram files`, the logical byte total
of unique committed Telegram chunk payloads. Reused chunk references are
counted once; Telegram protocol, message, and encryption-envelope overhead are
not included, and the value is derived from existing manifests without a
database migration.

The Overview's Network usage card has `This session` and `Total` tabs. The
session tab resets when the process starts; the total tab reports the four
payload counters accumulated across all server runs and stores them in metadata
schema v14. Both tabs refresh every five seconds and exclude protocol and
transport overhead.

## Required Runtime Settings

- chunk size
- connection timeout
- request timeout
- transfer timeout
- retry count
- retry backoff
- flood-wait respect
- local data directory

## Permission Checks

- session and key files must not be world-readable
- startup should reject obviously unsafe permissions when feasible

## Rotation

- rotate Telegram API credentials only from the authenticated admin panel and
  persist the new settings before restarting the process if needed. The admin
  save path rejects malformed numeric identifiers before writing them to
  `metadata.sqlite`; connection refresh failures are surfaced as JSON API
  warnings for operator correction after persistence succeeds.
- A Telegram connection removal hides the local namespace immediately. When
  remote deletion is selected, the owning generation remains reserved until
  cleanup completes; cleanup targets are never sent through a replacement
  account. Without remote deletion, the generation detaches immediately.
- rotate S3 credentials independently of Telegram session material
- rotate encryption keys via versioned envelopes

## Validation Notes

- `doctor` validates required credentials, runtime settings, the SQLite
  metadata path, the object-format bootstrap state, the Telegram transport,
  the RustFS-backed S3 seam in live mode, and the loopback admin listener
  address. Telegram bootstrap values are read from persisted admin settings.
  The storage peer must resolve to a reachable private channel or group chat;
  a plain Telegram user id will not work for object uploads.
- `server` performs the same bootstrap checks before binding the S3 listener
  and starting request processing. It also binds the loopback admin listener
  for `/healthz` and `/metrics`, while the authenticated operator frontend is
  served from the main listener under `/_admin`. If Telegram storage
  reconciliation cannot run because the storage peer is missing or unhealthy,
  startup stays up and the admin surfaces the degraded state instead of
  exiting. Storage chat ids are normalized to the signed Telegram dialog form,
  so `5582642885` is stored as `-5582642885` and `-1001234567890` stays as-is.
- The Docker image uses the same `server` path for foreground startup; the
  container entrypoint runs `config check` first and then starts the server in
  the foreground.
- Session and metadata paths are checked for unsafe `..` traversal, symlinks,
  and overly permissive permissions when the platform exposes those checks.
- `TELEGRAM_S3_MASTER_KEY` enables adapter-bound envelope encryption for chunk
  payloads and manifest encryption metadata. If it is missing, startup fails.
