# Testing

## Test Layers

1. Pure unit and state-transition tests
2. Manifest-format, encryption, checksum, chunk, and range tests
3. Database, migration, lease, fencing, and cleanup-outbox tests
4. Deterministic ambiguous-send and recovery-required tests
5. Mock Telegram reconciliation and corruption tests
6. Restart and subprocess recovery tests
7. RustFS adapter and standard-client S3 compatibility tests
8. Admin/API and authentication tests
9. Playwright Chromium operator-flow tests
10. Docker/Compose and preserved-volume tests
11. Opt-in real Telegram acceptance tests through the local SOCKS5 proxy

## Required Coverage

- failure before first chunk
- failure during a chunk
- failure after chunks but before manifest commit
- failure after manifest creation but before local commit
- process termination during multipart completion
- `recovery_required` with a null lease and a still-`sending` attempt
- lease loss while an acknowledgement is ambiguous
- exact token plus encrypted-byte reconciliation
- token collision with different bytes
- complete scan with no matching remote document
- Telegram unavailable, `AUTH_KEY_UNREGISTERED`, flood wait, timeout, and proxy disconnect
- missing Telegram message
- corrupt manifest
- corrupt chunk
- database lock or disk-full condition
- tombstone-first deletion and shared-message cleanup protection
- account/connection removal and restart recovery
- conditional S3 operations, multipart races, and bounded range reads

## Real Telegram Smoke Tests

- disabled by default
- require an explicit environment flag
- use a dedicated private test channel
- use unique test prefixes
- clean up only known-owned data
- default to dry-run cleanup if ownership is uncertain

## Suggested Commands

When code exists, run:

```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace --all-features

# Full deterministic release suite, including frontend and browser checks
./scripts/release-test.ps1

# Add the isolated live Telegram drill before an RC or stable release
./scripts/release-test.ps1 -LiveTelegram
```

The release runner skips the live drill unless `-LiveTelegram` is supplied.
Live tests require `TELEGRAM_LIVE_TESTS=1`, dedicated metadata/data paths whose
names contain `live-test`, a dedicated private storage chat, and the local
SOCKS5 proxy at `socks5://localhost:12334`. They never use the repository's
ignored `data/` directory.

The Playwright suite starts a local Vite server automatically. To point it at a
running authenticated admin server instead, set `PLAYWRIGHT_BASE_URL`.
