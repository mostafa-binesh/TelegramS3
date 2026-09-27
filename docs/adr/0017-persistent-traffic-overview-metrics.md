# ADR 0017: Persistent traffic totals in the operator overview

## Status

Accepted

## Context

The Overview's traffic panel originally exposed only process-local atomics. That
was useful for diagnosing a running server but reset whenever the process was
restarted, so operators could not compare current traffic with the server's
historical payload volume.

## Decision

- Keep the process-local counters as the `This session` view.
- Persist four lifetime payload counters in the metadata database and expose
  them as the `Total` view: client upload/download and Telegram
  upload/download.
- Store decimal counter values in a single metadata row so the counters remain
  exact for the full `u64` range and can be updated atomically under the
  metadata connection lock.
- Add the counters through metadata schema v14. Migration initializes missing
  totals at zero and does not modify manifests, transfer jobs, or object data.
- Count the same payload bytes already used by the session metrics; exclude
  protocol, framing, and transport overhead.

## Consequences

- The all-time view survives process restarts and metadata-backed restores.
- A metadata write is performed for each accounted payload increment, trading
  some SQLite write overhead for durable accounting and no loss on normal
  process restarts.
- The counters describe application payload bytes, not network-interface bytes
  or a Telegram account quota.
