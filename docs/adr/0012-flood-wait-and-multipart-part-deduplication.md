# ADR-0012: Preserve Telegram Pacing and Canonical Multipart Part Jobs

## Status

Accepted.

## Context

Telegram may reply to `messages.sendMedia` with `FLOOD_WAIT`. That is an
explicit RPC rejection before Telegram accepts a document, unlike a timeout or
connection loss where the acknowledgement is unknowable. Treating both cases
as `unknown` forced harmless rate-limit responses into manual recovery.

The multipart worker previously published multiple chunks concurrently. An
error from one future caused collection to cancel a sibling future while its
Telegram send was still in flight, leaving a stale `sending` attempt. Separately,
an S3 client retrying `UploadPart` after a delayed HTTP response could enqueue
another durable job for the same upload ID and part number. Those duplicate jobs
could pile up behind the first job and made active-transfer UI state unstable
during activity-poll failures.

## Decision

- Record explicit `FLOOD_WAIT` RPC replies as `retryable`, park the job in
  `retry_wait`, and wait for Telegram's requested duration (or a conservative
  fallback when no duration is provided).
- Upgrade historical unknown flood-wait attempts to retryable only by their
  recorded error. Preserve the scheduled delay even if a separate ambiguous
  attempt must be reconciled first.
- Publish one Telegram chunk at a time per durable transfer. This trades some
  per-transfer parallelism for a complete, recoverable outcome for every send.
- Treat the earliest active job for an upload ID and part number as canonical.
  A repeated in-flight `UploadPart` drains its duplicate request body and waits
  for that job instead of staging or sending another payload.
- Supersede a duplicate only when it has no checkpointed or ambiguous Telegram
  send. A job that may own remote documents stays under the existing
  reconciliation and evidence-first cleanup boundaries.
- Keep the operator browser's last successful multipart-activity snapshot when
  only the activity endpoint has a transient failure. The visible row therefore
  remains mounted while committed object listing data continues to refresh.

## Consequences

- Rate limiting no longer appears as a generic **Needs attention** failure and
  a resumed worker does not immediately violate the preserved flood-wait delay.
- A retrying S3 client does not create another Telegram upload for the same
  active part, but an intentional replacement must wait until the active part
  job reaches a safe terminal state.
- Timeout, disconnect, byte mismatch, and process-crash cases remain
  recovery-required until exact-token and exact-encrypted-byte reconciliation
  proves whether a remote document exists.
- In-flight UI rows remain operational status only; they still do not create a
  committed S3 object or download action before final manifest publication.
