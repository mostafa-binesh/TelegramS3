# ADR-0008: Isolate Telegram connection generations

## Status

Accepted for the Phase 10 lifecycle implementation.

## Decision

Persist an active Telegram connection-generation identifier in local metadata.
Copy that identifier to connection-removal jobs and cleanup outbox targets.
Removal hides the local namespace immediately. When remote deletion is
requested, the owning generation and transport remain reserved until Telegram
acknowledges evidence and message cleanup; without remote deletion, the active
generation is detached immediately. Cleanup code must compare the target
generation with the removal owner and quarantine mismatches instead of using a
replacement transport.

The login wizard is successful only when the transport health check is
`connected`, including storage-peer resolution. Session reuse or Telegram
authorization alone is not sufficient. `AUTH_KEY_UNREGISTERED` maps to
`needs_reauth` so the operator gets a repairable state rather than a false
success toast.

`TELEGRAM_SESSION_PATH` is the explicit deployment override; the metadata-
derived path remains the safe fallback. The SQLite schema is version 8 and
keeps the generation/outbox state restart-safe.

## Consequences

- A replacement account cannot accidentally delete documents belonging to a
  removal job that still owns its cleanup transport.
- A remote cleanup target can remain quarantined until an operator restores or
  otherwise authorizes the old generation; this is safer than guessing.
- The current single-connection process reserves reconnect until remote cleanup
  completes; future multi-user support must replace this reservation with
  per-account transport and credential snapshots rather than reusing global
  state.
- Operators must monitor `remote_cleanup_pending` and `recovery_required`
  targets; the system does not claim that an ambiguous Telegram deletion was
  completed.
