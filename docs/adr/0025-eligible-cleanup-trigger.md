# ADR-0025: Manual trigger for eligible cleanup

- Status: accepted
- Date: 2026-09-30
- Release: 0.7.6-rc.20

## Context

Cleanup retention is a per-target safety window, not a periodic scan interval.
The durable worker already sleeps until the next target is due and wakes when
new cleanup work is enqueued. Operators need a way to ask it to re-evaluate
eligible work immediately without turning retention into a destructive override.

## Decision

Add an authenticated `POST /_admin/api/cleanup/eligible-now` action exposed as
**Run eligible cleanup now** in Telegram settings → Storage policy. The action
starts the existing worker if needed and signals its existing wake notification.
No database migration or new cleanup implementation is introduced.

The worker's SQLite claim predicates remain authoritative: targets must already
be due, evidence must be complete before message cleanup, active reads and
shared references are respected, and `recovery_required` targets are never
claimed. The endpoint returns `202 Accepted` because cleanup continues
asynchronously.

## Consequences

- Operators can re-check due work without waiting for the worker's bounded wake
  delay.
- Clicking the action never shortens a retention window or retries quarantined
  data.
- The UI can report that work was queued, but completion remains visible through
  the existing cleanup metrics and recovery views.
