# ADR 0005: Authenticated operator frontend

## Status

Accepted

## Context

Phase 7 kept the Docker deployment to a single runtime container and left the
loopback admin listener for health and metrics. Phase 8 needs an operator
surface that can show storage overview, endpoint details, capacity, Telegram
readiness, and first-run guidance without exposing sensitive bootstrap state
before authentication.

The project also needs to stay honest about what is and is not part of the S3
compatibility surface. Operator visibility is useful, but it should not blur
the object-store contract or imply extra S3 semantics.

## Decision

- Serve the operator console from the same Rust process as the S3 server.
- Reserve the `/_admin` path prefix on the public listener for the authenticated
  SPA and JSON API.
- Keep `/healthz` and `/metrics` on the loopback-only admin listener.
- Use an HTTP-only cookie session gated by a bootstrap secret for first access.
- Build the frontend as a Svelte + Vite SPA and copy the built assets into the
  production image during the Docker build.

## Consequences

- Operators get a browser-based dashboard without adding another runtime
  container or a second compose service.
- Sensitive bootstrap material stays off browser storage and is never rendered
  before auth succeeds.
- The frontend can guide first-run setup and connection checks while the CLI
  remains available for Telegram login and recovery flows.
- The S3 compatibility matrix stays focused on S3 behavior, while operator UI
  behavior is documented separately.
