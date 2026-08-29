# Threat Model

## Assets

- Telegram API credentials
- Telegram session material
- S3 credentials
- Object ciphertext and plaintext
- Local metadata database
- Encryption keys and recovery material

## Trust Boundaries

- S3 client to server
- server to local metadata database
- server to Telegram transport
- local process to OS key store
- local process to filesystem permissions

## Primary Adversaries

- remote S3 clients sending malformed requests
- hostile or flaky network paths
- Telegram flood-wait and disconnect conditions
- local malware or another local user with file access
- corrupted local metadata or interrupted writes

## Key Risks

- partial uploads becoming visible
- stale index state after a crash
- secret leakage in logs or test output
- range-read corruption
- false positives from missing or corrupt Telegram messages
- proxy fallback masking direct-connect failure

## Mitigations

- operation journaling
- manifest-based reconstruction
- bounded buffering
- explicit retry classification
- secret redaction
- permission checks at startup
- dry-run repair and garbage-collection modes

## Residual Risks

- Telegram is still a third-party service with rate limits and file-size limits.
- Strong transactional semantics cannot be guaranteed end-to-end.
- Recovery depends on keeping both local metadata and remote manifests healthy.

