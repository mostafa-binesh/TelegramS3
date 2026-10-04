# Disaster Recovery

## Local Metadata Lost

Schema v20 account, scoped-replication, maintenance, download-failover, and
integrity-event rows
are part of the
recovery boundary.
Back up `metadata.sqlite` before adding or scheduling replication. Restoring
the database restores account definitions, replica/access maps, pending job
progress, re-chunk replica policy/target snapshots, re-chunk locks, and durable
per-account integrity findings; queued
workers resume after restart. Inspect
failed replication or re-chunk jobs before retrying them. An access-only record
is not a second copy: it is recoverable only while the target session can read
the shared Telegram chat. During recovery and ordinary reads, the persisted
source peer is resolved with the message ID; the target account's own storage
chat is not substituted for an access location.

The runtime opens the restored file through a bounded eight-connection SQLite
pool. Each handle uses WAL, foreign keys, `FULL` synchronous durability, and a
30-second busy timeout. A low-level failure discards and replaces one handle;
this does not make a corrupt or inaccessible database recoverable. After a
restore, run the normal `db status`, `index verify`, and application health
checks rather than relying only on the pool being able to open a connection.
The Accounts/Connections view loads this restored account state on entry and
has an explicit refresh action; it does not continuously poll account metadata
while the page is open.
The Profile page changes only the operator row in this same database. A display
name is restored with the user record, while a password is stored only as an
Argon2id hash; changing it advances the user's token version and invalidates
older sessions. No Telegram payload or object manifest is involved.

The admin bucket-level re-chunk action expands selected buckets into their
committed objects and queues independent durable jobs. A restart can therefore
 resume or inspect each object job separately; it does not create a single
 all-or-nothing bucket transaction.
Each job creates its replacement receiving transfer before staging chunks, so
the staging reservation is durable from the first chunk; the persisted
completion count includes a final partial chunk.

For a re-chunk job with “apply to replicas” enabled, schema v18 records the
source account and the old ready replica/access targets before replacement.
After the new primary manifest commits, the worker queues one-time follow-up
replication jobs for those targets. A crash before those jobs finish leaves the
new object readable through its primary account and the durable replication
queue visible for retry. A primary-only job deliberately does not carry old
locations to the new chunk layout; replicate the object again if alternate
download paths are needed.

The v17 account migration adds `download_enabled` with a default of `1`, so
existing accounts remain usable without rewriting manifests or replica rows.
Turning it off only changes future read selection. It does not detach the
account, change the owning connection recorded in cleanup targets, or delete
remote data. After restoring metadata, verify the account cards and re-enable
any account intended to serve reads before testing public or S3 downloads.

The v19 migration adds `telegram_download_failover_retries` to `app_settings`
with a default of `1`. It controls complete read retries on the selected
eligible account before the reader rotates to another replica location; it does
not change upload ownership or cleanup credentials. Restoring metadata restores
this policy, so verify it in Telegram settings after a restore if download
behavior needs to be changed.

Replica rows written after the v19 format change include their canonical
`chunk_size`. The admin bucket/object listing flags a non-zero mismatch against
the manifest as a yellow account badge. A missing value on an older replica is
treated as unknown and does not itself mark the object corrupt; re-replicate the
object to refresh that metadata.

For legacy replication jobs, `object_keys_json` is migrated to an empty array,
which preserves their original whole-bucket scope. New selected-key jobs keep
their exact key list durably, so a restart cannot silently expand a bulk
selection into a whole-bucket copy.

1. Stop the server.
2. Preserve the current Telegram session and data directories.
3. Restore `metadata.sqlite` from backup if you have one, then run `telegram-s3 db status` to confirm the metadata path and schema.
4. Rebuild the local index with `telegram-s3 index rebuild`.
5. Verify object counts and checksum samples with `telegram-s3 index verify`.

Expiry policy is recoverable because `expires_at` is embedded in each manifest,
not only in a browser or SQLite index row. Rebuilding the index therefore
preserves which objects are hidden. An expired object may briefly have Telegram
payloads until the background expiry sweep creates an evidence-first tombstone
and cleanup/GC removes them;
do not treat an expired object as proof that its remote bytes were already
deleted.

Share-link records are local metadata. The token hash is sufficient to validate
an already-issued public URL, while the encrypted token ciphertext lets the
authenticated admin panel display and manage current links after restart. Back
up `metadata.sqlite` together with `TELEGRAM_S3_MASTER_KEY`; without the same
master key, link URLs cannot be revealed in the manager. Older hash-only links
remain revocable but cannot have their URL reconstructed.

> The same `metadata.sqlite` now also stores operator accounts, Telegram
> bootstrap settings, connection-generation state, and session tombstones
> (schema v8). A backup/restore of
> that file restores both object
> state and who can sign in. If accounts are lost, re-provision the first
> operator with `telegram-s3 users create <username> --password <pw>` (the first
> account becomes the superadmin). Password hashes are argon2id + per-account
> salt and are not recoverable from the file alone. The object rows also hold
> the Telegram peer/message/document references, so losing the metadata store
> means losing the index that points at the Telegram payloads.

## Telegram Manifest Lost

1. Mark the object corrupt or unrecoverable.
2. Attempt to recover from a surviving metadata backup or from the source data
   that produced the object.
3. If the committed Telegram document is missing, treat the object as corrupt
   until it is repaired or re-uploaded.

## Interrupted Upload

1. After the S3 and admin listeners bind, the background recovery worker
   inspects the operation journal and begins the recovery snapshot.
2. The recovery pass now promotes complete staged uploads,
   recreates recovery markers for incomplete rows, and quarantines orphaned
   staging artifacts.
3. Resume only if the upload state is safe to continue.
4. Otherwise roll back and clean up staging or quarantined artifacts.

The initial recovery snapshot may validate committed chunks against Telegram
when the persisted first-scan-on-startup policy is enabled. If that policy is
disabled, the first remote scan waits for the configured verifier interval.
That remote work is intentionally asynchronous: a slow or unavailable
Telegram connection must not keep a restarted container listening only through
Docker's port proxies without the application accepting requests. Until the
scan completes, the admin recovery view reports that the recovery scan is
scheduled or pending with the next interval visible; committed-object
visibility remains governed by the local metadata index and the normal recovery
rules below.

### Interrupted Admin Browser Reception

The admin resumable upload API only resumes browser-to-server reception. The
browser keeps the reception ID and re-reads the authoritative offset after a
failed PATCH, but the server holds the active reception map in memory. Resume
only while the receiving lease is alive; after a server restart or expired
lease, re-select the source file and start a new reception. Completed
receptions are independent durable transfer jobs and continue through the
normal worker/reconciliation path.

The authenticated bucket browser polls those durable jobs while a bucket is open.
An active S3 key can therefore remain visible as an uploading row with completed
and total part counts, even though it is not yet a committed/downloadable object.
While a foreground folder listing is still loading, the browser keeps the last
successful rows visible with a loading affordance, and the silent activity poll
waits for that request instead of replacing its loading state. This prevents a
slow browser response from becoming an infinite skeleton without changing the
durable job or manifest state.
If the row changes to `recovery_required`, preserve the Telegram payloads and
metadata until reconciliation determines whether the acknowledgement can be
matched exactly; do not delete the visible Telegram files just because the browser
request was interrupted.

The authenticated browser uses bounded bucket and object listing pages so a large
namespace does not require rendering every entry at once. Bucket-name search and
recursive object-key search are read-only metadata queries; object search results
carry their parent location so an operator can navigate there before using the
normal download, share, link, or delete actions. Search and pagination do not
alter manifests, active-object pointers, transfer jobs, or recovery state. The
browser can sort bucket columns and folder object columns; the server sorts the
complete metadata set before pagination, so this presentation feature does not
change recovery ordering or object visibility.

Row-level Actions menus use the same durable server operations as bulk actions.
Selecting **Move** still publishes destination manifests and schedules source
cleanup without routing object bytes through the browser; selecting **Re-chunk**
creates the same durable lock and worker job for only that row. The UI keeps the
row target separate from any pre-existing bulk selection. Shared-link expiry
disclosures only change how the authenticated control is presented and do not
alter link tokens, object manifests, or cleanup evidence.

The object Information panel is a read-only inspection surface. It fetches the
active committed manifest on demand and shows the object identity, checksums,
encryption policy, chunk offsets, and alternate Telegram locations. Opening it
does not download object bytes, change recovery state, or acknowledge an issue.

The path-style names `_public` and `_admin` are reserved at bucket creation
because requests using those first path segments belong to the public-share and
admin routers. Recovery and index rebuild do not delete or rename any legacy
row with those names; the reservation only prevents new creation through S3 or
the admin console.

### Interrupted Public Download

Public and authenticated admin downloads are streamed from Telegram-backed
chunks rather than copied to a local whole-object buffer. A transient Telegram
RPC or transport failure while reading the current chunk is retried while the
response remains open for up to 120 seconds. The partially fetched chunk is
discarded, so no unverified bytes are sent to the client.

Normal downloads may prefetch a bounded number of additional complete chunks in
parallel. The policy is stored in metadata schema v13, defaults to one extra
chunk, and can be set from zero through four in Telegram settings. Prefetching
does not change object durability or manifest layout: each chunk is still
verified before delivery, output remains ordered, and outstanding work is
cancelled when the client stream ends. The adaptive scheduler starts small,
ramps up after clean reads, and backs off after retries or measured throughput
drops. A per-account limiter allows at most one active Telegram payload read per
account, while enabled physical replicas can still serve different chunks in
parallel. The persisted account-connection policy (schema v24, default `5`,
range `1–5`) caps each adaptive download further; the effective limit is the
smallest of that policy, the prefetch window, and the enabled replica accounts
with locations for the object. Sequential mode remains one remote read at a
time. These settings are runtime policies and do not change the manifest or
recovery boundary.

S3, public, and authenticated-admin segmented downloads also use a
process-local per-client/object gate with capacity one before the download
stage is created. Multiple range requests from one client IP for different
offsets of the same object wait behind the admitted response rather than
creating parallel stages, prefetch workers, or Telegram reads. The gate does
not change manifests, cleanup ownership, recovery state, or the behavior of
different objects. It serializes server work but cannot force a client to use
one TCP socket.

Operators may choose sequential nearest-chunk mode in the same policy page.
After the first requested chunk, it fetches the next chunks strictly in
manifest order, one at a time, with the prefetch value limiting how many
verified chunks may wait ahead of the client. This mode trades parallelism for
predictable account pressure and is still bounded, cancellable, and checksum
verified.

The first requested chunk is fetched before speculative prefetch begins to
reduce time to first byte. The authenticated Overview's Download stage metrics
section can be used while testing a read: it records bounded, process-local
samples for first-chunk, Telegram, retry-wait, decrypt, checksum-verify, and
total durations, plus retry and byte counts. These samples are diagnostic only
and disappear on restart; they are not recovery evidence or durable transfer
state. Active reads additionally expose the object, mode, current chunk,
client-delivered bytes, Telegram-read bytes, retries, and server/client chunk
counts, effective account limit, and eligible replica-account count from
process memory only.

Public and admin responses advertise `Accept-Ranges: bytes`. If the recovery
window is exhausted after earlier bytes were sent, a capable client can resume
with a single `Range: bytes=<offset>-` request. Missing messages, decryption
failures, and checksum mismatches are not retried as network failures; they
remain object recovery issues and must be handled through the recovery workflow.
Public audio/video links additionally use inline content disposition so media
players can issue their normal metadata and range probes. This is only an HTTP
response-header behavior; it does not change the stored manifest or Telegram
documents.

### Server-side admin moves

The admin move endpoint never sends source bytes through the browser or back
through Telegram. It expands selected folder prefixes from the local active
index, creates a durable destination transfer whose chunks reference the
original payload identity, publishes only the new JSON manifest document, and
only then tombstones the source. The published manifest is named with its
opaque send-attempt token, so an extensionless Telegram filename is expected;
reconciliation uses that token and exact bytes. Later evidence-first cleanup
may publish a separate JSON deletion-evidence document before deleting the old
manifest message. Neither operation re-uploads the encrypted chunk payload.
Directory markers are processed after their descendants so folder cleanup
preserves the existing non-empty-folder safety rule. A process failure can
leave a committed destination alongside an untouched source; this is
recoverable and does not silently discard data, but the operator must resolve
the destination conflict before retrying that item.

Automatic physical replication applies the same no-payload-copy optimization
when the source and target connections use the same Telegram storage chat:
the target account receives a ready location pointing at the existing message.
If the chats differ, replication falls back to the verified encrypted
download/upload path because the target chat needs its own physical message.

### Telegram Rate Limit (`FLOOD_WAIT`)

An explicit Telegram `FLOOD_WAIT` is safe to retry because Telegram rejected
the send before it published a document. The worker records it as `retry_wait`,
keeps the staging data, and resumes after Telegram's requested delay. The bucket
browser shows **Waiting for Telegram**, not **Needs attention**, while this is
in progress. Do not click retry repeatedly or start another copy of the same
part: a repeated in-flight S3 `UploadPart` joins the canonical durable job.

Timeouts, disconnects, and stale `sending` attempts are different: their remote
acknowledgement is unknown and they remain subject to exact-byte reconciliation.
When upgrading an older build, a historical flood-wait row is only moved to the
automatic retry path after every separate ambiguous attempt for that job has
been reconciled. This preserves both the pacing delay and the no-guessing
recovery boundary.

## Repair and Garbage Collection

1. Run `telegram-s3 repair --dry-run` first to see which staged, recovery-
   required, or orphaned rows will be reconciled.
2. Use `telegram-s3 repair` only after the dry-run shows the expected scope.
3. The cleanup worker drains tombstone outbox entries automatically. Local
   tombstones and orphaned cleanup material are retained for 12 hours by
   default before `gc` can remove them. The retention can be changed from
   Telegram settings for future delayed cleanup targets. Run
   `telegram-s3 gc --dry-run` when reviewing older tombstones or a manual
   cleanup scope; Telegram message removal remains evidence-first and retryable.
   The Storage policy page also offers **Run eligible cleanup now**; it wakes
   the same worker but processes only already-due targets and never bypasses
   retention or `recovery_required` quarantine.
4. Run `telegram-s3 gc` only when the dry-run output matches the intended
   cleanup scope.

The admin object browser follows the same recovery boundary. A delete response
is successful only after the local active pointer is tombstoned, so the item is
hidden from subsequent listings immediately. Deleting a folder with active
children is rejected; delete the children first, then remove the empty folder
marker. Telegram documents may remain until the background cleanup worker has
completed its evidence-first work.

## Removing the Current Telegram Connection

The admin Connection tab requires confirmation before removal. The transaction
immediately hides buckets, active objects, recovery markers, and related
statistics, while the durable `connection_removal_jobs` record keeps the scope
restart-safe. Recovery-required manifests, cancelled transfers, multipart
sessions, and local attention rows owned by that generation disappear from the
panel immediately; their operational records are purged after safe
finalization. Selecting **Also delete all uploaded Telegram files** enqueues
manifest and chunk messages for the evidence-first cleanup worker. The owning
Telegram generation and transport remain reserved until that queue completes;
this prevents the worker from deleting through a replacement account. Each
target stores its connection generation; if that generation is no longer
owned by the removal job, the worker quarantines the target instead of guessing
which account may authorize it.
Unknown Telegram acknowledgements remain `recovery_required` and keep evidence
material until the worker can inspect them. The worker records a unique send
token before upload, searches Telegram history back to the attempt timestamp,
and repairs a checkpoint only for an exact encrypted-byte match. If no match is
found after a complete scan, it schedules the chunk for retry; connectivity,
scan-limit, missing-staging, and byte-mismatch cases remain recovery-required.
On restart, a job already in `recovery_required` with no lease also has stale
`sending` rows normalized to `unknown` before reconciliation. That
normalization is committed even when no new transfer is available to claim, so
the recovery queue cannot roll back and leave Retry permanently blocked.
If the checkbox is not selected, local data is still removed from the active
installation but remote Telegram files are intentionally left in place.
Do not describe those retained files as deleted or recoverable through the
removed connection.

After cleanup completes, reconnect and verify that the overview reports `connected`, not merely
`authorized` or `reused`. A `needs_reauth` state with
`AUTH_KEY_UNREGISTERED` means the Telegram session is no longer valid and must
be logged in again; it is not a storage-peer lookup problem that a new delete
request will fix.

## Sampled Recovery Verifier

When enabled, the verifier runs once after workers start and then at the
configured interval. It can be disabled from Telegram settings; this stops
automatic remote checks without deleting existing recovery markers or changing
the saved interval/sample policy. For every healthy committed object it selects a fresh uniform random sample of
the configured number of distinct chunk indexes. The admin overview shows the
next run, sample policy, distinct broken-file count, and the current verifier
problem list.
If startup scanning is deferred, the Overview schedules and displays the next
first scan from the configured interval even though no scan has completed yet.

Each verifier run logs its start and completion (or failure) with elapsed
duration. The Overview also reports total scans, failed scans, last start/end
timestamps, and the last scan duration. These are runtime diagnostics and are
not required to rebuild object state.

Operators can use **Run integrity check now** in Telegram settings → Storage
policy to wake the verifier worker immediately. The worker owns the run and
restarts the configured interval after it completes; if the verifier is
disabled, the action is rejected and no scan is started.

If a sampled message is confirmed missing, cannot be decrypted, or fails its
checksum, the verifier records the account, chunk, failure, and repair state in
the durable integrity-event log. A healthy physical replica is used as the
source of the exact encrypted bytes for an automatic re-upload to the damaged
location. If no alternate can be verified, an unrecoverable primary issue marks
the object `recovery_required` and hides it from the active S3 namespace; a
replica-only issue does not hide an otherwise healthy object. Access-only
locations cannot be re-uploaded because they are shared-chat pointers and are
left retryable or isolated according to the failure. Temporary Telegram or
network failures remain retryable and do not mark the file broken.

## Interrupted Multipart Upload

1. Check `telegram-s3 auth status` first if the session was refreshed around
   the same time as the failure.
2. Multipart sessions are durable in local metadata, so restart reuse should
   preserve the upload ID and uploaded parts.
3. A row at `N/N` parts may be in `completing`: all payload chunks are already
   durable and the server is publishing the small final manifest. Current
   completion does not download or re-upload the concatenated object.
4. A **Waiting for Telegram** row is automatically pacing an explicit
   `FLOOD_WAIT`; leave its canonical job in place rather than uploading the
   same part again.
5. If a multipart session is marked `recovery_required`, abort or repair it
   before trying to complete the upload.
6. If the session files are gone but the local session row remains, clean up
   the multipart metadata and retry the upload from a fresh initiate call.

After upgrading from a release that reassembled multipart data during
completion, retry `CompleteMultipartUpload` with the same upload ID and ordered
part ETags. Abandoned receiving/queued completion jobs with no ambiguous remote
send are safely superseded by metadata-only composition. A job with a
`sending`/`unknown` attempt is not superseded: exact-byte Telegram
reconciliation must finish first so the server never guesses whether a remote
document exists.

The bucket browser may show the session as receiving, uploading, finalizing, or
needing attention during these steps. That row is deliberately not a committed
object: it provides part progress and recovery state, but has no download or
object actions until the final manifest is published.

## Docker Deployment Loss

1. Stop the `telegram-s3` container before restoring state.
2. Preserve the named metadata, data, and session volumes together if the
   container is still healthy enough to inspect.
3. Recreate the container from the same `docker-compose.yml` and `.env`
   settings so the bind addresses and volume mounts remain consistent. If the
   Telegram bootstrap settings were already saved in the admin panel, they come
   back with the `metadata.sqlite` volume.
4. If only the image is lost, rebuild it and reattach the preserved volumes.
5. After restore, run `telegram-s3 doctor` or
   `docker compose exec telegram-s3 telegram-s3 doctor` to confirm the
   bootstrap path before returning traffic.
6. Use the authenticated `/_admin` dashboard to recheck storage overview,
   capacity, Telegram readiness, bucket visibility (including buckets created
   from the UI), and bootstrap status before resuming writes.
   The Overview's `This session` client/Telegram traffic counters start a new
   baseline after the restored server process starts. The `Total` counters are
   stored in metadata schema v14 and survive the restart; they are operational
   payload accounting, not durable object-recovery evidence.
   The Overview's Telegram-files size is manifest-derived and remains
   meaningful after restart, but it represents logical committed chunk bytes,
  not Telegram protocol overhead or a remote account quota.
   Storage counts and this manifest-derived size are served from a short-lived
   five-second cache. Routine Overview polling uses `/overview/live` for
   transfer, traffic, stage, and connection telemetry; a full Overview
   refresh repopulates the storage snapshot.
7. If a console view remains on a loading skeleton, inspect the response for
   its hashed file under `/_admin/assets/`. Rebuild/redeploy the image with the
   complete UI `assets/` directory, then use the view's Retry action; this is a
   UI-asset issue and does not alter the persisted Telegram settings.
8. If an administrative action reports `invalid csrf token`, the current SPA
   automatically re-reads the session cookie and retries that action once. If
   the retry still fails, treat it as a real session expiry/revocation and sign
   in again; do not disable CSRF protection or delete metadata to repair it.

The Telegram account wizard owns an operator-scoped flow id. Leaving the wizard
cancels that flow, and reopening it requests a fresh code. Settings are persisted
together before sign-in begins; an invalid code keeps the current attempt retryable,
and an expired code requires starting a new attempt. Stale browser requests cannot
cancel or advance a replacement flow. The wizard changes only bootstrap settings
and login state; it does not change object manifests, chunks, or recovery markers.

## Telegram Session Loss

1. Keep the local metadata and data directories intact.
2. If the session file is missing or invalid, run `telegram-s3 auth login` to
   create a fresh Telegram session.
3. Confirm the new session with `telegram-s3 auth status` and the `/_admin`
   overview card, which should move to connected once the storage chat lookup
   succeeds.
4. If Telegram rejects the session, use `telegram-s3 auth logout` and log in
   again with a fresh code.

## S3 Server Bootstrap Failure

1. Run `telegram-s3 doctor` to check the shared bootstrap path.
2. If doctor fails before the listener binds, inspect the Telegram session,
   proxy settings, bucket rows, and recovery markers first.
3. If admin-side Telegram settings were changed shortly before the failure,
   verify that the persisted API ID and storage chat ID are numeric before
   retrying transport startup or login.
4. Fix the underlying object-format or Telegram transport issue before
   retrying `telegram-s3 server`.
5. A successful restart should preserve committed objects, keep staged work
   invisible, only make repaired data visible after reconciliation, and bind
   the loopback admin listener for `/healthz` and `/metrics`.
6. The same restart should also keep the authenticated `/_admin` operator
   frontend available for readiness and recovery checks.
7. Verify bucket and object visibility through the client class you depend on
   (`ListObjects` legacy callers as well as `ListObjectsV2` callers) before
   returning the endpoint to backup tooling.

The admin connection-removal action is intentionally guarded by the linked
Telegram account identity. The admin UI displays the stored phone number and
requires the operator to type that exact number before removal. If the number
is unavailable, do not bypass the check; use the normal Telegram
reauthorization flow first so the account identity can be stored again.

## Orphan Cleanup

- Run garbage collection in dry-run mode first.
- Never delete uncertain data without operator confirmation.
- Keep an audit summary of the cleanup scope.
- The live `gc` command only removes tombstoned data that is safely past the
  retention threshold; uncertain data stays quarantined.

## Recovery Commands

- `telegram-s3 doctor`
- `telegram-s3 auth status`
- `telegram-s3 auth logout`
- `telegram-s3 auth login`
- `telegram-s3 db status`
- `telegram-s3 db migrate`
- `telegram-s3 index rebuild`
- `telegram-s3 index verify`
- `telegram-s3 repair`
- `telegram-s3 gc --dry-run`
