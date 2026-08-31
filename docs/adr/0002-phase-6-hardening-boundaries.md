# ADR 0002: Phase 6 Hardening Boundaries

## Status

Accepted

## Context

Phase 6 hardens the production surface without expanding the public S3 API.
The project needs envelope encryption for chunk payloads, a conservative repair
and garbage-collection path, and operator visibility that does not mix admin
traffic into the S3 listener.

## Decision

- Encrypt chunk payloads with adapter-bound envelope encryption keyed from
  `TELEGRAM_S3_MASTER_KEY`.
- Record encryption metadata in manifests so committed objects remain
  rebuildable and auditable across restarts.
- Keep chunk reads bounded by decrypting only the requested spans, not by
  buffering entire objects in RAM.
- Bind `/healthz` and `/metrics` on a separate loopback-only admin listener.
- Keep `repair` and `gc` conservative: `--dry-run` first, then reconcile or
  clean only when the metadata state is unambiguous.

## Consequences

- The S3 listener remains focused on object traffic while operators get
  loopback-only bootstrap and recovery visibility.
- Recovery now depends on both local metadata and the manifest encryption
  metadata.
- Garbage collection is intentionally slower and safer, but it avoids deleting
  uncertain data.
