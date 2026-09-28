# ADR-0020: Bounded metadata connection pool

- Status: accepted for 0.7.6-rc.12

## Context

The metadata store previously guarded one `rusqlite::Connection` with a
process-wide mutex. That made every metadata read and write wait behind the
same handle, and a low-level handle failure could make the whole control plane
unusable until restart.

## Decision

Keep SQLite as the authoritative local metadata and journal store, but open a
bounded pool of eight independently configured connections. File-backed
connections use WAL, foreign-key enforcement, `FULL` synchronous durability,
and a 30-second busy timeout. Existing metadata methods continue to lease one
connection for the duration of each synchronous callback, so transaction
boundaries and recovery semantics remain unchanged.

When a metadata operation reports a low-level I/O, corruption, open, or
read-only error, the failed handle is discarded and the pool immediately
attempts to replace it. Normal constraint, validation, busy, and locked errors
remain ordinary operation errors; they do not cause unnecessary handle churn.

## Consequences

- Concurrent reads no longer serialize behind one mutex.
- SQLite still serializes writers, so pooling is not a promise of parallel
  write throughput.
- One bad handle no longer requires taking down the whole metadata service.
- The pool is runtime-only; existing databases need no migration and remain
  readable by the same schema version.
- A corrupt or inaccessible database file still requires the documented backup
  and recovery procedure; replacing a handle cannot repair the file itself.
