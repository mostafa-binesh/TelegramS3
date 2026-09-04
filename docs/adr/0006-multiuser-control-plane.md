# ADR 0006: Multi-user accounts, sessions and the control-plane file surface

## Status

Accepted (initial slice landed; the bounded binary content streaming, Telegram-backed byte storage, and the in-browser Telegram onboarding wizard have since landed as follow-up increments, see the `## Update` section below).

## Context

Phase 8 shipped an operator console where the only login gate was a shared
`TELEGRAM_ADMIN_BOOTSTRAP_SECRET` taken from `.env`: any session cookie was
HMAC-signed with that one value, had no user identity, and there was no way to
add or remove people who should reach the management UI. Phase 9 needs:

- a real, database-backed multi-user model where admins manage accounts (no
  per-user `.env`), with a username/password barrier in front of *every* data
  API and nothing content-visible to guests;
- per-user session lifecycle with logout, expiry and revocation on password
  change or account deletion;
- rate limiting / lockout on login;
- a CLI path to provision the first superadmin while the server is down;
- a file-browser-like management surface over the existing S3 object data.

## Decision

- **Accounts live in the same `metadata.sqlite`** as object state (schema bump
  `3 → 4`), not a separate credential file. One single-writer store keeps one
  backup/restore unit (`docs/disaster-recovery.md`), one `db migrate` surface,
  and the server/CLI can share the opened store.
- **Passwords are hashed with argon2id** (PHC strings, OWASP baseline
  `m=2^16,t=3,p=4`) in a pure `auth` crate module shared by the admin HTTP layer
  and the `users` CLI. Argon2 runs off the async reactor via a blocking thread;
  per-age the salt comes from `rand_core`/OS RNG. Unknown usernames perform a
  real argon2 verify (timing equalization) so enumeration is not trivially
  observable.
- **Session cookies carry user identity** (`uid`, per-session `sid`, `token_version`,
  `iat`, `exp`, nonce `csrf`) and are HMAC-SHA256 signed with a server cookie
  secret. The env `TELEGRAM_ADMIN_BOOTSTRAP_SECRET` is no longer a *login*
  credential; when present it is used only to derive the cookie-signing key (so
  existing image deploys keep working). Session rows are inserted at issue time
  so a single session can be revoked; `users.token_version` bump revokes every
  session of that user atomically (password change / delete / disable).
- **Guest gating**: `/_admin/api/session` is the only guest-visible endpoint (a
  safe whoami). Every other API returns `401` without a valid principal.
- **Rate limiting / lockout**: an in-process fixed-window limiter keys on the
  IP (when available) and on the attempted username, with escalating lockout.
  Chosen over `governor` because the surface is one interactive control-plane
  endpoint in a single process and `governor` would add heavy machinery for no
  benefit; reset on successful login.
- **Roles are stored but reserved**: every account is admin-tier in this phase;
  the value `role` (`admin` | `superadmin`) is normalized and authorizers accept
  across all admins, with `superadmin` reserved for account CRUD. Per-user
  buckets/tenancy and per-file ACLs are future work. The very first account
  created is forced to `superadmin`, so the CLI can never strand the system
  without a privileged operator.
- **Folders are a view model over `/`-delimited keys** (S3-native), with an
  optional zero-byte directory-marker object only to persist an *empty* folder
  that must survive; list-time grouping never requires a marker. This mirrors
  the S3 data plane and does not add unsupported S3 semantics.
- **The browser surface is split**: control-plane core (login, users, and a JSON
  bucket/folder/object listing + folder create/delete + tombstones) landed with
  this slice. Binary content streaming (upload/download/range), Telegram-backed
  object bytes, and the in-browser Telegram onboarding wizard are implemented
  in follow-up increments described in `## Update` below. Bulk/folder download
  remains future work.
- **Storage-op authority**: the management controller talks to the same
  `ObjectFormatService` as the S3 server (single-writer, bounded reads,
  tombstones before cleanup). It never introduces an independent object-writing
  path, and it never buffers a full object in memory.

## Consequences

- `.env` no longer enumerates users; account admin is in-app and first-boot is
  a documented `telegram-s3 users create` CLI step.
- A leaked arbitrary cookie can be invalidated on logout; password changes are
  instantly effective. Cookie `Secure` becomes mandatory once the deployment
  places the listener behind TLS — this is surfaced as configuration guidance.
- Schema is now `4`; additive migration only, existing object tables untouched.
- Backup/restore of `metadata.sqlite` now also restores operator accounts and
  session tombstones — losing it means re-running `telegram-s3 users create`.
- AGENTS / compat documentation must keep `/_admin` (control plane) distinct
  from the S3 compatibility matrix; object visibility gained through `/_admin`
  is still the same committed S3 object data and stays governed by S3 rules.

## Update (content streaming + in-browser Telegram onboarding)

Implemented as the documented follow-up increment:

- **Bounded upload** (`POST /_admin/api/objects/content?bucket=&key=`) is a
  transport bridge only: the raw request body is turned into an
  `s3s::dto::StreamingBlob` and handed to the existing S3 `put_stream`, which
  now commits the payloads as Telegram documents/messages. The write path is
  shared with the S3 data plane and memory stays bounded.
- **Shared streaming reader**: `ObjectFormatService::read_spans_to_stream`
  decrypts + checksum-verifies one chunk span at a time by fetching Telegram
  documents chunk-by-chunk. Both the S3 `get_object` and the admin
  `GET/HEAD /_admin/api/objects/content` endpoints feed this one reader (DRY
  and byte-for-byte identical S3 output). Full + range downloads answer
  `206`/`Content-Range` without buffering an object.
- **The wizard keeps exactly one single-account Telegram client alive while it
  is open**, opened lazily on `begin` and dropped on completion/cancel, so we
  avoid holding an idle client when Telegram is already authorised and no wizard
  is running. A process-wide `TelegramLoginDriver` enforces a **single in-flight
  flow** (a second operator's `begin` → `409`); the retained
  phone/sign-in/2FA token lives in the driver, not on the shared transport, and
  failures reuse the CLI's classification.
- **CLI parity**: both the headless `telegram-s3 auth login` and the HTTP wizard
  reuse the same classification and retry primitives on the transport, so
  failures (flood waits, expired/invalid codes, wrong password) surface and
  retry identically. The CLI keeps its interactive `interactive_login` wrapper
  (stdin/stdout prompts) driving those shared real steps rather than changing
  behaviour for headless/CI use; aligning it to literally call in to the driver
  object is a documented, non-functional follow-up.
- **Honest scope gate**: there is still no bulk folder download/server-side ZIP
  (which would require whole-object buffering). That remains recorded as future
  work in `docs/limitations.md`.

## Rejected alternatives

- Shared bootstrap-secret-as-login: no identity, no per-account revocation, weak.
- Per-user `.env` accounts / no in-app admin: contradicts the stated model.
- Separate `auth.sqlite`: two write paths and two backup units for little gain.
- Stateless-only sessions without the `admin_sessions` row: cannot revoke a
  single session; only "drop all by rotating the global signing secret".
- Introducing `governor`: overkill for one login endpoint; not durable anyway.
- Homogeneous site/peer replication of Telegram S3 against RustFS: unsupported
  (requires a full MinIO/RustFS admin peer; see repo docs for the linkage).
