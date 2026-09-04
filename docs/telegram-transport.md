# Telegram Transport

Phase 2 turns the Telegram layer into a real transport/service boundary. The
code now owns session reuse, login state, proxy resolution, and a shared retry
policy for Telegram RPC calls.

## Commands

- `telegram-s3 auth login` performs headless phone-code login and prompts for an
  optional 2FA password when Telegram requires it.
- `telegram-s3 auth status` reports the configured session path and current
  Telegram/auth state.
- `telegram-s3 auth logout` signs the session out and leaves the local session
  path in place for the next login.
- `telegram-s3 doctor` and `telegram-s3 server` fail fast when the transport
  cannot be initialized in live mode.

## Session Reuse

- The configured `TELEGRAM_SESSION_PATH` is the persisted Telegram session
  database.
- Startup reuses that path automatically when it already exists.
- A successful login writes back to the same session file so subsequent process
  starts can reopen it.
- The authenticated `/_admin` panel uses the same persisted session and hot-
  reloads the live transport after a successful Telegram reauthorization so
  storage writes can resume without a server restart.

## Proxy Selection

The transport resolves proxy behavior from the existing env contract:

- `TELEGRAM_PROXY_MODE=direct` disables proxy use and rejects a proxy URL.
- `TELEGRAM_PROXY_MODE=socks5` uses the provided `socks5://` URL directly.
- `TELEGRAM_PROXY_MODE=auto` accepts `socks5://`, `http://`, and `https://`
  URLs:
  - `socks5://` is used directly
  - `http://` and `https://` are bridged through a local SOCKS5 listener
- `TELEGRAM_PROXY_USERNAME` and `TELEGRAM_PROXY_PASSWORD` are applied to the
  resolved upstream proxy credentials.

Invalid combinations are rejected before the CLI proceeds.

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
