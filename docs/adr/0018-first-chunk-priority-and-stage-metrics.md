# ADR-0018: First-chunk priority and read-stage diagnostics

- Status: accepted
- Date: 2026-09-27

## Context

The shared Telegram-backed reader can prefetch verified chunks in parallel,
but opening the full look-ahead window immediately can delay the first bytes
when Telegram is slow or the client is probing a public media link. Operators
also need evidence about whether a delay is in Telegram, retry waiting,
decryption, checksum verification, or client delivery without turning every
download into a durable database record.

## Decision

Fetch the first requested span in a single-item stream before opening the
configured `prefetch + 1` window. Subsequent spans continue to use the existing
ordered, bounded prefetch policy and are emitted only after complete fetch,
decryption, and checksum verification.

Record a bounded, process-local sample for each shared stream. The sample
identifies the `public`, `admin`, `s3`, or test surface and includes first-chunk
latency, Telegram time, retry count and wait time, decrypt time, checksum
verification time, total duration, chunk count, and client/Telegram payload
bytes. Keep only the latest 20 completed or failed samples and expose them in
the authenticated Overview as a testing section. Do not persist stage samples
in SQLite or use them as recovery evidence, object accounting, or traffic
totals.

## Consequences

- Public and private reads reach first-byte delivery without waiting for the
  entire speculative window to settle.
- The prefetch setting continues to control only the bounded work after the
  first span; output order, memory bounds, and integrity checks are unchanged.
- Operators can compare stage timings during a live test, while process-local
  retention avoids per-chunk metadata writes and does not expand the durable
  recovery model.
- A restart clears the diagnostics; durable traffic totals and recovery state
  remain the sources of truth for their respective purposes.
