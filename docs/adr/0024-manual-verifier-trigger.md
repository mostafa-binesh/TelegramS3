# ADR-0024: Manual recovery-verifier trigger

- Status: accepted for 0.7.6-rc.19
- Date: 2026-09-30

## Context

The sampled integrity verifier runs on a configured interval. Operators need a
way to check the current Telegram state immediately after a repair, migration,
or connectivity change without waiting for that interval. Running the scan
directly from the HTTP handler would create a second execution path and could
race the background worker.

## Decision

Add an authenticated `POST /_admin/api/recovery/verify-now` action exposed as
**Run integrity check now** on Telegram settings → Storage policy. The request
signals the existing verifier worker through a wake notification. The worker
continues to own scan execution, logging, metrics, snapshots, and recovery
handling. After the manual scan finishes, the worker starts a new configured
interval, so the manual run resets the next scheduled scan. The action returns
`409 Conflict` while the verifier is disabled and does not start a scan.

This is an in-memory scheduling signal only; it does not add a database column
or migration and does not change manifest, recovery-event, or verifier-policy
serialization.

## Consequences

- Operators can run an immediate integrity check without restarting the server
  or waiting for the next interval.
- Manual and automatic scans share exactly the same sampling and recovery code.
- A queued manual run remains asynchronous, so the settings request stays
  responsive while Telegram reads are in progress.
- The existing startup policy, interval, sample count, disabled state, and
  durable recovery findings remain unchanged.
