# ADR 0022 - Cleanup Queue Scheduling and Retention

## Status

Accepted for 0.7.6-rc.16

## Context

The cleanup outbox is intentionally conservative: a Telegram message is not
removed until deletion evidence has been durably accepted. The previous claim
query scanned dependent evidence rows for every one-second idle poll. With a
large backlog this made an otherwise idle server consume CPU, while
`recovery_required` rows correctly remained excluded from automatic deletion.

## Decision

- Add indexes for cleanup claim ordering and `(object_id, target_kind,
  completed)` dependency checks.
- Prefer evidence claims, then message claims whose evidence is complete.
- When no target is actionable, sleep until the next known window, capped at
  one minute. New cleanup-producing operations signal the worker directly so
  due work starts immediately without restoring one-second polling.
- Persist a unique cleanup-evidence attempt token and timestamp before every
  remote evidence upload.
- Reconcile ambiguous attempts by exact token and exact encrypted/evidence
  bytes. Only a complete scan proving absence may return the target to pending.
  A match completes the evidence target; errors and byte collisions remain
  `recovery_required`.
- Store cleanup retention in `app_settings`, defaulting to 12 hours and
  allowing one hour through 30 days. It applies to future delayed/orphan
  cleanup targets and does not rewrite existing scheduled or quarantined rows.

## Consequences

The idle cleanup path no longer performs a full dependency scan every second.
Deleting an object still hides it locally immediately, while physical Telegram
cleanup remains evidence-first. Historical recovery-required rows without an
attempt token cannot be safely guessed through and require operator-led
reconciliation.
