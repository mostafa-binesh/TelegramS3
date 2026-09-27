# ADR 0016: Telegram payload size in the operator overview

## Status

Accepted

## Context

Operators need a quick indication of how much data is represented by the
Telegram-backed store. The object manifests already record every committed
chunk's remote Telegram location and logical plaintext size. Multipart
composition can reuse a remote chunk in more than one manifest, so summing
object sizes would overstate the payload retained by Telegram.

## Decision

- Expose `storage.telegram_files_bytes` in the authenticated overview API.
- Sum the logical sizes of unique remote chunk locations referenced by
  committed manifests. Reused multipart payloads count once.
- Exclude Telegram message/protocol overhead, encryption-envelope bytes, and
  uncommitted or recovery-required objects. The value is an operational
  manifest-derived payload estimate, not a Telegram account quota or billing
  measurement.
- Derive the value during the existing object-format status scan; no metadata
  schema or migration change is needed.

## Consequences

- The Overview can show a stable total without issuing a remote Telegram scan
  or adding another durable counter that could drift from manifests.
- The displayed value is intentionally lower than exact Telegram account
  storage because transport and encryption overhead are excluded.
- Tombstoned, staged, and recovery-required objects do not inflate the
  committed Telegram payload total.
