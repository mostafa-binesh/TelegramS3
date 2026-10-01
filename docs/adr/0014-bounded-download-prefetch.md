# ADR-0014: Bounded download prefetch

- Status: accepted
- Date: 2026-09-27

## Context

The download path must fetch a complete Telegram document, decrypt it, and
verify its checksum before it can send any bytes to an S3, admin, or public
client. That integrity boundary is necessary, but it can produce a visible
zero-speed gap between adjacent chunks when the next Telegram read starts only
after the previous chunk has been delivered.

## Decision

Use an ordered, bounded futures buffer around the shared stream reader. The
database-backed `telegram_download_prefetch_chunks` setting controls the number
of extra chunks in flight, accepts `0–4`, and defaults to `1`. The effective
concurrency is `prefetch + 1`, so the current chunk and only the configured
look-ahead window can be active. Results are emitted in manifest order even if
later Telegram requests finish first.

The configured value is a ceiling for an adaptive window. The reader starts
with a small window, grows it after clean reads, and backs it off after retries
or measured throughput drops. A per-account semaphore permits only one active
Telegram payload read for an account; different enabled physical replicas may
therefore provide parallel capacity without multiplying pressure on one
account. Download stage diagnostics record the final/max window and per-account
bytes, retries, chunks, and Telegram duration.

Every prefetched result must complete remote download, decryption, and checksum
verification before it enters the output stream. Client traffic counters are
incremented only when a verified result is emitted. A stream error terminates
the stream and dropping the stream drops outstanding prefetch futures.

Schema migration v13 creates the setting for existing databases with the
default value. The policy is runtime-only for stream scheduling; it does not
change manifest layout, existing object chunking, active upload boundaries, or
recovery semantics.

## Consequences

- Normal chunk-boundary stalls are reduced without buffering an entire object.
- `0` provides a serial-read fallback for constrained Telegram accounts or
  memory-limited deployments.
- Higher values can improve client smoothness but increase parallel Telegram
  traffic and transient per-stream memory, bounded by four extra chunks.
- A missing, corrupt, or permanently unavailable chunk remains an integrity
  failure; prefetching does not hide or repair it.
