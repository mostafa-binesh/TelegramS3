# Testing

## Test Layers

1. Pure unit tests
2. Manifest-format tests
3. Database and migration tests
4. Crash and recovery tests
5. Property-based tests for chunk and range mapping
6. Fault-injection tests
7. RustFS adapter integration tests
8. S3 compatibility tests with a standard client
9. Proxy-path tests with a local proxy
10. Opt-in real Telegram smoke tests

## Required Coverage

- failure before first chunk
- failure during a chunk
- failure after chunks but before manifest commit
- failure after manifest creation but before local commit
- process termination during multipart completion
- missing Telegram message
- corrupt manifest
- corrupt chunk
- flood wait
- timeout
- proxy disconnect
- database lock or disk-full condition

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
```

