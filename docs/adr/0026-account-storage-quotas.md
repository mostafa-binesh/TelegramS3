# ADR-0026: Per-account storage quotas

## Status

Accepted in `v1.0.1-rc.1`.

## Context

Telegram storage accounts are the ownership boundary for primary objects and
physical replicas. Operators need a local capacity policy that can stop a
particular account from receiving more data without pretending Telegram offers
transactional quota enforcement or billing information.

## Decision

Store a nullable `quota_bytes` on each Telegram account. `NULL` means unlimited.
Derive usage from durable local metadata rather than maintaining a mutable
counter: sum committed active primary manifest lengths owned by the account and
ready physical replica chunk sizes; exclude access-only pointers.

All receive surfaces perform a cheap admission check before accepting a new
receive. The transfer worker checks again before sending staged bytes to
Telegram, and the manifest commit transaction performs the authoritative final
check. Replacing an active object at the same key consumes only the positive
size delta. A rejected staged transfer is marked `reception_failed`, retains an
explicit error for the transfer UI, and is cleaned by the normal local staging
cleanup path. S3 maps the policy error to `EntityTooLarge`; admin endpoints use
`507 Insufficient Storage`.

The admin editor accepts GiB values or unlimited mode. Overview displays every
account's used/limit state in a five-column desktop grid and responsive fewer-
column layouts.

## Consequences

- Usage is rebuildable from manifests and replica rows after metadata recovery.
- Concurrent receives can stage locally, but the final transactional check
  prevents an over-quota committed object.
- Telegram protocol overhead, Telegram's own service limits, and access-only
  references are outside this local quota.
- Exact reservation of unknown-length streaming bodies is intentionally not
  introduced; capacity is enforced before remote publication and at commit.
