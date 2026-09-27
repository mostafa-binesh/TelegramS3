# ADR-0019: Account pool, bucket replication, and re-chunking

- Status: accepted for 0.7.6-rc.6
- Date: 2026-09-27

## Context

One Telegram connection limits throughput and makes a single remote location a
single point of failure. Operators also need to share a bucket with another
authorized Telegram session without necessarily uploading another copy, and
change chunk policy for existing objects without exposing a mixed manifest.

## Decision

Keep the existing active connection as the compatibility owner, and add an
additive account registry keyed by immutable connection ID. Persist replica and
access locations per object/chunk. Physical replication copies encrypted bytes
through the target connection; access replication records the source location
and requires the target session to have chat access. The reader rotates across
the primary and ready physical locations per chunk.

Replication and re-chunking are durable jobs. Re-chunking stages a replacement
object through the existing transfer journal, locks the old object for reads,
and publishes the replacement only after all chunks are committed. Existing
manifests remain backward-compatible because replica lists are optional.

## Consequences

- Existing databases migrate without rewriting buckets or manifests.
- A target account needs its own authorized session file; the same Telegram
  account may be registered more than once only with independent sessions.
- Access-only mode is not redundancy and fails when the target session cannot
  resolve the shared group.
- Automatic replication is a scheduled durable job, not a Telegram-side
  transaction. Operators must monitor failed jobs and recovery state.
- Re-chunking temporarily returns an explicit retry-later response for the
  object, preserving consistency over availability during the swap.
