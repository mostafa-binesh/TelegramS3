# Telegram Transport

Phase 2 turns the Telegram layer into a real transport/service boundary. The
code now owns session reuse, login state, resolved bootstrap settings persisted
in `metadata.sqlite`, and a shared retry policy for Telegram RPC calls.

## Commands

- `telegram-s3 auth login` performs headless phone-code login and prompts for an
  optional 2FA password when Telegram requires it.
- `telegram-s3 auth status` reports the resolved session path and current
  Telegram/auth state.
- `telegram-s3 auth logout` signs the session out and leaves the local session
  path in place for the next login.
- `telegram-s3 server` can start before Telegram is configured so the
  authenticated admin panel can collect bootstrap settings.
- `telegram-s3 doctor` reports missing Telegram bootstrap settings until they
  are saved in the admin panel.

## Session Reuse

- The Telegram session path is derived automatically from the metadata path as
  `<metadata-dir>/telegram.session`.
- Startup reuses that path automatically when it already exists.
- A successful login writes back to the same session file so subsequent process
  starts can reopen it.
- The authenticated `/_admin` panel uses the same persisted session and hot-
  reloads the live transport after a successful Telegram reauthorization so
  storage writes can resume without a server restart.

## Retry Policy

- Telegram RPC calls share one retry policy driven by
  `TELEGRAM_RETRY_COUNT`, `TELEGRAM_RETRY_BACKOFF_MS`, and
  `TELEGRAM_FLOOD_WAIT_RESPECT`.
- Flood waits are parsed from the Telegram error text and honored when the
  respect flag is enabled.
- The policy is shared by the transport boundary so future upload/download
  paths can reuse the same behavior.

## Test Mode

- The repository smoke tests use `TELEGRAM_TRANSPORT_RUNTIME=mock` to avoid
  live network access.
- Mock mode keeps the CLI shape intact while using local-only session handling
  for test coverage.

## Bootstrap Settings

- Telegram API credentials, storage chat id, and proxy settings are edited in
  the authenticated admin panel and stored in `metadata.sqlite`.
- The CLI and server resolve those settings from the persisted store on startup
  and again when the admin panel saves a change.
- The environment no longer carries Telegram API credentials, storage chat id,
  session path, or proxy settings.
