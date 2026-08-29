# ADR 0003 - Metadata and Consistency

## Status

Accepted

## Context

Telegram is not a transactional object store. The project needs recoverable
behavior for partial uploads, overwrites, deletes, and listings.

## Decision

Use a durable local SQLite journal as the fast path and recovery ledger, while
the canonical manifest stored in Telegram remains the rebuild source after local
loss.

Write path:

1. create operation record
2. stage chunks
3. verify checksums
4. upload or update manifest
5. atomically commit local metadata
6. expose object to readers
7. schedule cleanup

## Consequences

- partial uploads stay invisible
- overwrites preserve the previous committed object until the new one commits
- deletes become tombstones before cleanup
- startup reconciliation is required

## Rejected Alternatives

### Telegram as the only metadata store

Rejected because recovery and listing would be too weak.

### Best-effort writes without a journal

Rejected because interrupted writes would leak partial state.

