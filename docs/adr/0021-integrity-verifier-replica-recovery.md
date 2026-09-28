# ADR-0021: Integrity verifier replica recovery

- Status: accepted for 0.7.6-rc.13

## Context

The sampled verifier previously checked only the canonical primary Telegram
location. A damaged physical replica could therefore remain in round-robin
read selection until a client happened to use it, and a healthy alternate
could not be used to restore it automatically.

## Decision

For every sampled chunk, verify the primary and every recorded replica through
the account transport that owns or can access that location. Persist one
integrity event per account/chunk in schema v20, including the failure,
account, repair state, and operator-facing details.

When a physical primary or replica fails but another location verifies the
canonical plaintext checksum, copy the verified encrypted bytes to the failed
physical account and update the manifest location. Access-only locations are
shared-chat pointers and are not re-uploaded. A replica-only failure does not
quarantine an otherwise healthy object. A primary failure is still marked
`recovery_required` when no alternate can be verified or the repair cannot be
completed.

Known-dead replica locations are removed from future read selection only when
the failure is confirmed missing/corrupt and no repair source exists. Temporary
transport failures remain in place and are retried by later verifier scans.

## Consequences

- Replica damage is visible in the Recovery UI with account and chunk context.
- Physical replicas can self-heal without re-encrypting or buffering the whole
  object; the immutable encrypted chunk bytes are copied as-is.
- The metadata database gains a durable integrity-event table and requires the
  additive schema v20 migration.
- An access-only account outage cannot be fixed by uploading another document;
  it remains a retryable access/transport issue.
