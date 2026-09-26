# ADR-0013: Sampled recovery verification for committed objects

## Status

Accepted.

## Context

The original background recovery snapshot downloaded every chunk of every
committed object on each fixed 60-second pass. That creates avoidable Telegram
traffic and makes large stores look as if they are constantly downloading from
Telegram. The server also cannot recreate a missing remote chunk from a local
copy after committed staging has been cleaned.

## Decision

The verifier has a database-backed enabled flag and operator-configurable
interval. Each
healthy committed object contributes a uniformly random sample of distinct
chunk indexes, capped by the number of chunks in that object. The sample count
and interval are exposed in the Storage policy page and overview telemetry.

Confirmed missing messages, decryption failures, and checksum mismatches move
the object to `recovery_required`, remove it from the active index, and persist
the problem details in a recovery marker. The verifier never silently reuploads
the missing data. Temporary Telegram/network read errors remain
`verification_unavailable` and are retried later without changing object
visibility. Full repair remains an explicit operation and can restore an
object only when the existing remote payload passes verification or a trusted
source is supplied.

## Consequences

- Normal verifier traffic is bounded by the configured sample count per object,
  rather than the total chunk count.
- Sampling is probabilistic across scans; checking every chunk requires setting
  the sample count at least as high as the largest object or running explicit
  repair.
- A confirmed integrity failure becomes visible in the overview and Recovery
  page and fails closed for S3 reads/listings.
- Existing databases import environment defaults once; SQLite settings are
  authoritative after import.
- Operators can disable automatic verification without deleting the saved
  policy or existing recovery findings; the overview reports the disabled
  state and no next run is scheduled.
