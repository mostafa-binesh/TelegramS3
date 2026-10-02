# ADR-0019: Account pool, bucket replication, and re-chunking

- Status: accepted for 0.7.6-rc.11
- Date: 2026-09-27

## Context

One Telegram connection limits throughput and makes a single remote location a
single point of failure. Operators also need to share a bucket with another
authorized Telegram session without necessarily uploading another copy, and
change chunk policy for existing objects without exposing a mixed manifest.

## Decision

Keep the existing active connection as the compatibility owner, and add an
additive account registry keyed by immutable connection ID. Persist replica and
access locations per object/chunk. Physical replication reuses the source
encrypted message when the source and target connections use the same storage
chat; different-chat replication copies encrypted bytes through the target
connection. Access replication records the source location and requires the
target session to have chat access. The reader rotates across
the primary and ready physical locations per chunk.

Replication and re-chunking are durable jobs. Re-chunking stages a replacement
object through the existing transfer journal, locks the old object for reads,
and publishes the replacement only after all chunks are committed. Existing
manifests remain backward-compatible because replica lists are optional.

## Consequences

- Existing databases migrate without rewriting buckets or manifests.
- A target account needs its own authorized session file; the same Telegram
  account may be registered more than once only with independent sessions.
- Access-only mode is not redundancy and fails when the target session cannot
  resolve the shared group.
- Automatic replication is a scheduled durable job, not a Telegram-side
  transaction. Operators must monitor failed jobs and recovery state.
- Re-chunking temporarily returns an explicit retry-later response for the
  object, preserving consistency over availability during the swap.

## Follow-up: read eligibility and connection workspace

Each account also stores a `download_enabled` flag. It is deliberately a read
policy, not an ownership or cleanup policy: disabling an account removes its
primary, replica, and access locations from client download selection while
leaving uploads, immutable ownership, and evidence-first cleanup unchanged. If
no eligible location remains, the reader fails closed instead of silently using
a disabled credential.

Re-chunking remains object-scoped. It publishes a replacement manifest with a
new chunk layout; replica locations attached to the old manifest are not
silently reused because their message bytes no longer map to the replacement
chunks. The old manifest and its replica locations follow the existing
evidence-first cleanup path. Operators must queue replication again when
alternate physical copies are required after re-chunking.

The bucket browser supports selecting multiple bucket rows. Its re-chunk action
expands each selected bucket into its committed object keys and queues the same
one-object-per-job workflow; the storage layer remains intentionally
object-scoped so locks and progress are independently recoverable.

## Follow-up: replica-aware re-chunk policy

Re-chunking now presents an explicit operator choice. Schema v18 snapshots the
source account and each ready replica/access target in the durable job. With
“apply to replicas”, the replacement manifest commits first and then queues
one-time follow-up replication jobs for those targets. With “primary account
only”, no old location is attached to the replacement layout; the object is
available through its primary account, but round-robin replica reads resume
only after a new replication job completes.

The additional-account UI uses the same four-step wizard as the primary
connection, but passes the immutable account ID through the entire login flow
so API credentials, session files, and Telegram code/password operations stay
isolated from the primary account.

The admin UI treats the primary and additional connections as one account
workspace. Account cards share one add/edit form, and Overview derives one
aggregate state (`connected`, `partial`, or `disconnected`) from per-account
health while exposing named indicators for each connection. Account transports
are cached after their first health probe so recurring Overview refreshes do
not rebuild every secondary transport.

## Follow-up: replica layout visibility and read failover

Replica locations now persist the canonical source chunk size. The admin API
compares that value with each manifest chunk and marks the object and containing
bucket when a non-legacy replica layout differs. The UI uses a yellow account
badge for that warning; a missing value in an older manifest remains unknown
and is not treated as corruption.

The shared reader keeps the deterministic per-chunk account rotation, but a
failed selected account receives the configured number of complete retries
before the next eligible account is attempted. The
`telegram_download_failover_retries` database setting defaults to one retry and
accepts zero through eight. Each attempt retains the existing transport retry,
120-second stream recovery, decryption, and checksum verification boundaries;
accounts are never mixed within one chunk.

Each selected location is read through its persisted Telegram peer and message
ID. This is required for access-only replicas: the target account may be
configured with a different storage chat while still having access to the
source chat. Message IDs are chat-scoped, so using the target account's
configured storage chat would make a valid access replica unreadable.
