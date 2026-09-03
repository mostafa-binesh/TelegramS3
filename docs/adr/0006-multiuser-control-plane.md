# ADR 0006: Multi-user accounts, sessions and the control-plane file surface

## Status

Accepted (initial slice landed; streaming + browser Telegram wizard intentionally split into a follow-up increment).

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
  this slice. **Binary content streaming** (bounded upload via `put_stream` from
  an `Incoming` body and bounded/ranged download mirroring the S3 chunk stream)
  and the **in-browser Telegram onboarding wizard** (driving the single-account
  Telegram `Client` with a `request_login_code → code → (2FA password)` state
  machine) are isolated follow-up increments — see the Phase 9 section of
  `ROADMAP.md`.
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

## Rejected alternatives

- Shared bootstrap-secret-as-login: no identity, no per-account revocation, weak.
- Per-user `.env` accounts / no in-app admin: contradicts the stated model.
- Separate `auth.sqlite`: two write paths and two backup units for little gain.
- Stateless-only sessions without the `admin_sessions` row: cannot revoke a
  single session; only "drop all by rotating the global signing secret".
- Introducing `governor`: overkill for one login endpoint; not durable anyway.
- Homogeneous site/peer replication of Telegram S3 against RustFS: unsupported
  (requires a full MinIO/RustFS admin peer; see repo docs for the linkage).
