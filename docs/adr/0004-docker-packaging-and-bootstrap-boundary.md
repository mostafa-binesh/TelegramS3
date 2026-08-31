# ADR 0004: Docker packaging and bootstrap boundary

Status: accepted

## Context

Phase 7 adds production Docker packaging for Telegram S3. The project already
performs config validation, Telegram bootstrap, object-format bootstrap, and
loopback admin binding inside the Rust server process.

The Docker deployment needed to stay simple:

- one main service
- no separate setup container
- no second compose service for onboarding or bootstrap
- persistent metadata, data, and session state mounted on volumes
- S3 traffic exposed to the host while admin traffic stays loopback-only

## Decision

The Docker image will package the existing Rust binary directly.

- Container startup runs `telegram-s3 config check` and then `telegram-s3 server`
  in the foreground.
- Compose deploys a single `telegram-s3` service.
- The service exposes only the S3 listener on the host.
- The admin listener remains loopback-only inside the container.
- State lives on mounted volumes for metadata, chunks/manifests, and sessions.

## Consequences

- Operators get one deployment unit to manage instead of a bootstrap service
  plus a runtime service.
- Setup steps still happen through the same binary, using `auth login`,
  `doctor`, and `server`.
- The container image remains useful for both local development and production
  deployment.
- Future auth UI work can build on this packaging boundary without changing the
  deployment shape again.
