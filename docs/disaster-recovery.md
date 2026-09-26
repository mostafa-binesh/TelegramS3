# Disaster Recovery

## Local Metadata Lost

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

The initial recovery snapshot may validate committed chunks against Telegram.
That remote work is intentionally asynchronous: a slow or unavailable
Telegram connection must not keep a restarted container listening only through
Docker's port proxies without the application accepting requests. Until the
scan completes, the admin recovery view reports that the recovery scan is
pending; committed-object visibility remains governed by the local metadata
index and the normal recovery rules below.

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

### Interrupted Public Download

Public downloads are streamed from Telegram-backed chunks rather than copied
to a local whole-object buffer. A transient Telegram RPC or transport failure
while reading the current chunk is retried with the configured retry and
flood-wait policy. The partially fetched chunk is discarded, so no unverified
bytes are sent to the client.

Public responses advertise `Accept-Ranges: bytes`. If bounded retries are
exhausted after earlier bytes were sent, a capable client can resume with a
single `Range: bytes=<offset>-` request. Missing messages, decryption failures,
and checksum mismatches are not retried as network failures; they remain object
recovery issues and must be handled through the recovery workflow.

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
   tombstones and orphaned cleanup material are retained for 24 hours by
   default before `gc` can remove them. Run
   `telegram-s3 gc --dry-run` when reviewing older tombstones or a manual
   cleanup scope; Telegram message removal remains evidence-first and retryable.
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

If a sampled message is confirmed missing, cannot be decrypted, or fails its
checksum, the object is marked `recovery_required` and hidden from the active
S3 namespace. This is intentional: the server has no trustworthy plaintext
source from which to recreate that chunk. It preserves the manifest and all
remaining Telegram evidence, and an operator must re-upload or restore the
original source (or run repair if a later full verification proves the remote
payload is intact). Temporary Telegram or network failures remain retryable and
do not mark the file broken.

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
   The overview's client/Telegram traffic counters are process-scoped and
   start a new baseline after the restored server process starts; they are not
   durable recovery evidence.
7. If a console view remains on a loading skeleton, inspect the response for
   its hashed file under `/_admin/assets/`. Rebuild/redeploy the image with the
   complete UI `assets/` directory, then use the view's Retry action; this is a
   UI-asset issue and does not alter the persisted Telegram settings.

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
