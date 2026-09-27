# Changelog

## 0.7.6-rc.11 - 2026-09-28

### Replica layout mismatch visibility and download failover

- Persisted the source chunk size on every newly written physical or access
  replica location. Bucket and object APIs now report when a replica advertises
  a different non-legacy chunk size from the canonical manifest chunk, and the
  Buckets account badge changes to a yellow warning icon for that object or
  bucket. Legacy replica rows without this field remain an explicitly unknown
  layout and do not produce a false warning.
- Added a schema v19 migration for `telegram_download_failover_retries`, with
  a database-backed `0–8` setting and a default of one retry. The Storage policy
  UI validates, saves, reloads, and explains the setting.
- Updated the shared reader so a failed selected account receives the configured
  number of complete read retries before the reader rotates to the next
  enabled, ready account location. Every attempt still uses the existing
  Telegram retry/recovery window, decrypts and verifies the complete chunk, and
  never splits a chunk across accounts.
- Added Rust coverage for bounds, schema migration, persistence, and candidate
  rotation, plus Playwright coverage for the retry setting and yellow mismatch
  badge. Updated the storage-format, S3 compatibility, recovery, README,
  roadmap, and architecture decision documentation.

## 0.7.6-rc.10 - 2026-09-28

### Isolated additional-account onboarding

- Replaced the plain “Add account” editor path with the same four-step wizard
  used by the primary Telegram connection: API access, storage chat, network
  route, and Telegram authorization.
- Added an explicit account label in the additional-account wizard and styled
  the add tile as a deliberate onboarding card instead of a compressed inline
  control.
- Persist the newly created account before sending the Telegram login request,
  then carry its immutable account ID through code and cloud-password steps.
  The selected account retains its own session path, transport manager, and
  health state; primary account fields are no longer overwritten or restored
  when adding another account.
- Added browser coverage for isolated wizard values, account-specific login
  payloads, authorization completion, and subsequent download-policy editing.

### Replica-aware re-chunking

- Added schema v18 migration columns for the re-chunk source account, the
  apply-to-replicas decision, and a durable snapshot of ready replica/access
  targets. Existing databases migrate additively and existing jobs default to
  primary-only behavior.
- Added a confirmation choice to bulk/object re-chunking: apply the new chunk
  layout to the old replica/access targets, or run the faster primary-only
  replacement.
- When replica application is selected, the worker snapshots those targets
  before replacement and queues durable one-time follow-up replication jobs
  after the primary manifest commits. Physical copies are rebuilt and
  access-only locations are restored through the normal replication path.
- When primary-only is selected, old replica locations stay tied to the old
  manifest and are cleaned up with it. The replacement is readable through
  the primary account but has no replica download paths until re-replicated;
  the UI now states this consequence before confirmation.
- Added migration coverage and browser request assertions for the persisted
  replica policy.

## 0.7.6-rc.9 - 2026-09-28

### Multi-account connection workspace and read policy

- Refactored Connections into selectable account cards with an add-account tile
  and one shared add/edit form for the primary and additional accounts.
- Added persisted per-account `download_enabled` settings through additive
  schema v17 migration. Disabled accounts are excluded from primary, physical
  replica, and access-only chunk reads, while upload ownership and cleanup
  remain bound to their original account.
- Cached isolated account transport managers and health monitors so replica
  reads and overview refreshes do not repeatedly create Telegram clients.
- Changed Overview Telegram health to aggregate all accounts as connected,
  partial, or disconnected and added per-account green/red status dots with
  account-name hover labels.
- Refactored the overview health construction around a lightweight account
  snapshot and added safe live reload behavior when an account is edited.
- Increased folder listing Modified and Actions column widths and added a
  browser geometry regression test so timestamps and action icons cannot
  overlap.

### Sortable bucket and folder listings

- Added clickable Name, Created, and Accounts sorting to the bucket table and
  Name, Size, and Modified sorting to the inner folder object table.
- Sorting is sent to the authenticated listing API and applied before
  pagination, so changing pages preserves the selected global order. Sort state
  resets the view to page one and supports ascending/descending toggling.
- Added responsive table behavior and browser coverage for request parameters,
  rendered order, and narrow viewport layout.

### Bucket bulk selection and re-chunk scope

- Added checkbox selection to the bucket listing, including select-all-visible
  and fixed-bottom bulk actions for guarded deletion and re-chunking every
  committed object in the selected buckets.
- Selecting files inside a bucket still queues one durable re-chunk job per
  selected object, while selecting buckets enumerates their committed objects
  and queues the same durable jobs for each bucket.
- Documented that re-chunking replaces the source manifest and does not
  automatically rebuild existing physical replicas; the old replica locations
  remain under evidence-first cleanup and must be replicated again if needed.

## 0.7.6-rc.8 - 2026-09-27

### Unified account workspace and replication controls

- Moved the primary Telegram connection out of the storage settings page and
  into the Accounts workspace, so primary and additional accounts share one
  consistent connection UI.
- Added separate Connections, Replication, and Maintenance tabs under
  Accounts, with the primary account clearly identified and destructive account
  actions kept scoped to the selected connection.
- Added account/copy/access badges to bucket and object rows. The details dialog
  now shows physical replicas, access-only accounts, storage chats, chunk
  coverage, and the applicable replication controls.
- Added one-time or automatic replication controls for whole buckets and for
  selected object keys. Bulk replication uses the same durable job queue as the
  detail view.
- Added fixed-bottom bulk actions for replicate, move, delete, and re-chunk so
  selected-object controls remain available while browsing long listings.

### Session recovery and navigation reliability

- Expired authenticated sessions now synchronize state and return the operator
  to login instead of leaving an API error page that can only be repaired by a
  manual refresh.
- Search-result “open location” now preserves the recursive object search after
  navigation, so a result remains visible even when it is not on the first page
  of its containing folder.
- Preserved the existing listing during folder transitions and fixed replica
  detail rendering for accounts that have both physical-copy and access-only
  records for the same chunk.

### Download diagnostics and compatibility

- Added a dedicated Download stage metrics test action. It performs an isolated
  diagnostic read and reports its own sample, rather than reusing the latest
  ordinary client download.
- Added schema v16's additive replication object-key scope. Existing jobs keep
  whole-bucket semantics, while newly queued selected-key jobs persist their
  exact scope across restart and worker recovery.
- Added browser coverage for expired-session recovery, account relocation and
  tabs, replica/access detail rendering, bulk replication payloads, search
  navigation, and the standalone stage-metrics test; added migration coverage
  for preserving existing replication jobs.

## 0.7.6-rc.7 - 2026-09-27

### Replica account badge accuracy

- Corrected object and bucket account counts to include the active primary
  connection alongside physical replicas and access-only accounts.
- Split the Buckets badges into explicit `copies` and `access` counts so the
  account distribution is immediately clear.

## 0.7.6-rc.6 - 2026-09-27

### Account pool, replication, and maintenance queues

- Added schema v15 migrations for multiple Telegram connections, replica/access
  locations, durable replication jobs, re-chunk jobs, and temporary object
  locks. Existing connection IDs and manifests are preserved.
- Added an Accounts console section for independent Telegram transports,
  one-time or automatic bucket replication, and physical-replica versus
  access-only modes.
- Added durable replication progress and account-access detail APIs, account
  badges in Buckets, and a per-chunk account detail dialog.
- Added optional per-chunk replica locations and deterministic round-robin
  streaming across the primary and ready replica transports.
- Added worker-backed bulk re-chunking from the bucket browser. It streams
  chunks through bounded staging, publishes a replacement manifest through the
  transfer journal, locks reads while active, and reports progress.
- Added coverage for migration, account/job records, replica serialization,
  account navigation/forms, replication payloads, and re-chunk confirmation.

## 0.7.6-rc.5 - 2026-09-27

### Download diagnostics and first-chunk delivery

- Added a bounded, process-local **Download stage metrics** section to the
  authenticated Overview. It shows active, completed, and failed reads plus
  recent per-request timings for first chunk delivery, Telegram fetch time,
  retry wait, decryption, checksum verification, and total stream time.
- Metrics identify the read surface (`public`, `admin`, or `s3`), count chunks
  and payload bytes on each side, include Telegram retry counts, and retain at
  most 20 recent samples. They are diagnostic telemetry only: no per-download
  rows or stage timings are written to SQLite, and the recorder resets when
  the process restarts.
- Added a real-browser testing panel and Playwright coverage so an operator can
  start a download and inspect the stage breakdown after the five-second
  Overview refresh.
- Changed the shared reader to prioritize the first requested chunk before
  opening the configured speculative prefetch window. This reduces time to
  first byte while preserving the existing ordered, bounded `0–4` prefetch
  policy and checksum boundary.
- Added Rust coverage proving first-chunk ordering, prefetch behavior, stage
  sample accounting, and retry-count reporting.

### Bucket and object search and navigation

- Added numeric pagination and search controls to the bucket and folder
  browser, with recursive server-side object search across buckets.
- Search results now show their containing bucket/folder location and retain
  direct folder navigation plus download, share, link-manager, and delete
  actions against the canonical object key.
- Added browser coverage for search requests, pagination, result actions, and
  direct navigation to nested result locations.

## 0.7.6-rc.4 - 2026-09-27

### Bucket and object browser navigation

- Added bounded server-side pagination for bucket and folder listings, with
  total counts and next/previous navigation so large namespaces do not require
  rendering every entry at once.
- Added bucket-name search and recursive object-key search. Search results
  include their containing location and retain download, share, link-manager,
  delete, and direct folder-navigation actions.
- Added browser coverage for pagination, search request parameters, result
  actions, and direct navigation to a nested result location.

### Reserved internal bucket names

- Reserved the exact bucket names `_public` and `_admin` because those
  path-style prefixes belong to the public-share and authenticated-admin HTTP
  surfaces.
- S3 `CreateBucket` now returns `InvalidBucketName`, including for requests
  that would otherwise be intercepted by an internal route. The admin bucket
  form returns HTTP 400 with a clear validation message.
- Added shared-service, S3 integration, and browser regression coverage. This
  is a creation-time rule only: no metadata migration rewrites existing rows.

## 0.7.6-rc.3 - 2026-09-27

### Persistent Overview traffic tabs

- Split Overview network usage into `This session` and `Total` tabs while
  preserving the existing five-second refresh cadence and separate client and
  Telegram upload/download channels.
- Added metadata schema v14 with durable lifetime payload counters. Totals are
  restored after process restart and are updated from the same payload byte
  accounting used by the session view; protocol overhead remains excluded.
- Added migration, persistence/reopen, backend API, and browser tab-selection
  coverage. Existing object and transfer data are unchanged.

## 0.7.6-rc.2 - 2026-09-27

### Telegram storage size in Overview

- The Overview now shows the total logical payload represented by unique
  committed Telegram chunk files, formatted alongside the bucket/object
  snapshot metrics.
- Reused Telegram chunks are counted once, so multipart composition and
  manifest references do not inflate the displayed total. Telegram protocol,
  message, and encryption-envelope overhead are intentionally excluded.
- Added backend API, object-format, and browser coverage for the metric; it is
  derived from existing manifests and requires no metadata migration.

## 0.7.6-rc.1 - 2026-09-27

### Public media streaming

- Public audio and video links now use `Content-Disposition: inline` while
  retaining the original filename. This lets HTTP media clients such as
  PotPlayer treat a public MP4/MP3 URL as playable media instead of treating
  it only as an attachment download.
- Public links continue to send the correct stored media type,
  `Content-Length`, `Accept-Ranges`, and `Content-Range` headers, so players
  can probe media metadata and seek/resume through byte ranges. Non-media
  public objects remain attachments.

### Admin session resilience

- The admin SPA now detects the specific `invalid csrf token` response caused
  by a stale in-memory session token, re-reads the current guest-safe session,
  updates the live UI session, and retries the original request once. This
  covers JSON actions, raw object writes, browser uploads, and resumable upload
  chunks, so operators no longer need to refresh the page after a session
  rotation in another tab or browser context.
- CSRF validation remains enforced by the server. The client does not retry
  unrelated authorization failures, does not weaken the cookie/header match,
  and does not loop after the single recovery attempt.
- Added browser coverage proving that a rejected bucket action is retried with
  the refreshed token and succeeds without a page reload.

### Tests and documentation

- Added public media-header integration coverage and documented the streaming
  behavior and compatibility boundary across the README, roadmap,
  configuration, S3 compatibility, storage-format, disaster-recovery, and
  architecture decision documents.

## 0.7.5 - 2026-09-27

Stable release promoted from `v0.7.5-rc.3`.

### Bounded parallel download prefetching

- Added a database-backed `telegram_download_prefetch_chunks` policy for
  smoothing Telegram-backed downloads. The default fetches one additional
  chunk while the current chunk is delivered; setting it to `0` restores
  serial reads, and values up to `4` are accepted.
- The S3, authenticated admin, and public-share readers now prefetch only a
  bounded number of verified chunks concurrently, preserve manifest order,
  and emit client traffic counters only when bytes are actually delivered.
- Prefetched chunks are fully downloaded, decrypted, and checksum-verified
  before they can reach the client. A failure still aborts the stream rather
  than exposing partial or unverified bytes, and dropping a stream cancels the
  remaining prefetch work.
- Added Storage policy controls, preset values, validation feedback, and
  browser coverage for changing and persisting the prefetch window.

### Migration, tests, and documentation

- Bumped the metadata schema to v13. Existing databases receive the default
  prefetch value through an idempotent migration; existing manifests and
  active transfers remain unchanged.
- Added migration, settings round-trip, bounded-stream ordering, validation,
  admin API, and persistence coverage.
- Documented the memory/parallelism trade-off and download behavior across the
  README, roadmap, configuration, S3 compatibility, storage-format,
  disaster-recovery, metadata-store, and architecture decision documents.

## 0.7.5-rc.2 - 2026-09-27

### Download stream recovery

- Public-share, authenticated admin, and S3 download streams now keep the
  response open for up to 120 seconds while retrying transient Telegram,
  proxy-bridge, or transport I/O failures for the current chunk.
- Stream recovery is deadline-based rather than limited to the normal
  per-operation attempt count; a stalled Telegram read is also bounded by the
  same 120-second window.
- Authenticated admin downloads now advertise `Accept-Ranges: bytes`, matching
  public-share responses and allowing capable clients to resume with
  `Range`/`Content-Range` after the recovery window is exhausted.
- Permanent missing-message, decryption, checksum, and invalid-configuration
  errors still fail immediately and remain visible to recovery workflows.
- Added coverage for stream retries beyond the normal attempt limit and for
  resumable authenticated-download headers.

## 0.7.5-rc.1 - 2026-09-26

Release candidate for resilient Telegram-backed downloads.

### Download reliability

- Added bounded per-chunk retries for transient Telegram RPC and transport I/O
  failures while serving S3 and public-share reads. The retry uses the existing
  Telegram retry count, backoff, and flood-wait policy.
- Retries discard the incomplete remote fetch and restart the same Telegram
  document from the beginning. A chunk is emitted only after the existing
  decrypt and checksum verification succeeds, so a failed retry cannot leak
  partial or unverified bytes.
- Missing Telegram messages, decryption failures, checksum mismatches, and
  other permanent integrity errors remain non-retryable recovery failures.
- Added object/chunk/message identifiers to exhausted streaming-read warnings
  for production diagnosis.

### Resume support

- Public `/_public/<token>` responses now advertise `Accept-Ranges: bytes`.
- Existing single-range handling is covered as a resume path with `206`,
  `Content-Range`, and the correct remaining body bytes. `HEAD` responses
  advertise the same capability.
- No metadata migration is required; existing manifests, Telegram documents,
  and local databases remain compatible.

### Tests and documentation

- Added retry success and permanent-failure unit coverage.
- Extended public-share integration coverage for range resume and headers.
- Documented retry, integrity, resume, and recovery behavior in the README,
  roadmap, configuration, S3 compatibility, storage-format, disaster-recovery,
  and ADR documentation.

## 0.7.4 - 2026-09-26

Stable release promoted from `v0.7.3-rc.8`.

### Recovery verification

- Added a database-backed sampled recovery verifier. Each healthy committed
  object is checked at a configurable interval using a fresh, uniformly random
  sample of distinct chunk indexes, without downloading every chunk on every
  pass.
- Added Storage policy controls for verifier enabled state, verification
  interval, and random chunks per file. The verifier can be disabled entirely;
  saved policy values and existing recovery findings are preserved.
- Added `TELEGRAM_RECOVERY_VERIFY_ENABLED` as the configuration seed for new
  metadata databases. Existing databases reuse the existing `app_settings` and
  `recovery_markers` tables, so no schema bump or destructive migration is
  required.
- Confirmed missing, undecryptable, or checksum-invalid sampled chunks
  quarantine their object as `recovery_required`, remove it from active
  listings, and retain durable recovery details. Temporary Telegram or network
  failures remain retryable and do not quarantine objects.
- Added Overview verifier telemetry: enabled or disabled state, next scheduled
  run, sample policy, distinct broken-file total, and the current problem list.
  The Recovery page provides the full corrupted and missing-file issue list.

### Admin UI and storage policy

- Restored the complete Storage Policy layout after the recovery verifier
  controls were introduced: policy cards, two-column grids, styled input
  wrappers, unit suffixes, preset buttons, badges, action row, and impact panel.
- Added browser regression coverage for the storage-policy layout and verifier
  settings, including disabled-state behavior, persistence, and save failures.
- Preserved the existing file-management, folder-navigation, transfer,
  recovery, traffic-analytics, operator, and Telegram-settings improvements
  already included in `v0.7.3`.

### Documentation and validation

- Updated the configuration, disaster-recovery, S3 compatibility, Telegram
  storage-format, README, ROADMAP, and sampled-verification ADR documentation.
- Added Rust coverage for random sampling, quarantine behavior, migration
  preservation, recovery markers, and verifier lifecycle behavior.
- Validation completed with Rust formatting, Clippy, the full Rust workspace
  test suite, frontend type checks/build, and all 36 Playwright tests.

## 0.7.3-rc.8 - 2026-09-26

Done jobs:

- Restored the storage-policy page layout styles that were accidentally omitted
  when the recovery verifier controls were added. The chunk policy and verifier
  controls now render in their intended cards, grids, input wrappers, badges,
  preset buttons, action row, and impact panel.
- Added browser regression assertions for the storage-policy grid, card, and
  input-wrapper layout.
- Revalidated the complete frontend Playwright suite.

## 0.7.3-rc.7 - 2026-09-26

Done jobs:

- Added a database-backed sampled recovery verifier. Each healthy committed
  object is checked at a configurable interval using a fresh, uniformly random
  sample of distinct chunk indexes, without downloading every chunk on every
  pass.
- Added Storage policy controls for the verifier enabled state, interval, and
  random chunks per file. Disabling it stops automatic scans while preserving
  the saved policy and existing recovery findings.
- Added verifier status to Overview: enabled/disabled state, next-run timer,
  sample policy, distinct broken-file total, and the current problem list.
- Confirmed missing, undecryptable, or checksum-invalid chunks now quarantine
  the object as `recovery_required`, remove it from active listings, and retain
  durable recovery details. Temporary Telegram/network failures remain
  retryable and do not quarantine objects.
- Added the `TELEGRAM_RECOVERY_VERIFY_ENABLED` seed setting and documented that
  all verifier settings use existing metadata tables, so existing databases
  require no schema bump or destructive migration.
- Added Rust migration-preservation, random-sampling, quarantine, frontend
  settings, disabled-state, Overview, and regression coverage.

## 0.7.3 - 2026-09-26

- Promoted the validated `v0.7.3-rc.6` build to stable.
- Includes smooth folder navigation with stale-poll protection and regression
  coverage for slow responses, races, and bucket re-entry.
- Includes aligned bucket-table geometry, non-overlapping timestamps and action
  icons, narrow-screen support, and labeled bulk Delete and Move actions.
- Includes the nested-object deletion fix, polished operator account card,
  Recovery Telegram badge, and aligned destructive-action checkbox.
- Includes expanded Overview analytics and separate client/Telegram upload and
  download traffic counters refreshed every five seconds.
- Completed Rust, frontend, and 34-test Playwright verification for the release.

## 0.7.3-rc.6 - 2026-09-26

Done jobs:

- Smoothed folder navigation by keeping the last successful listing visible
  while the next prefix loads, preventing the browser from falling into an
  indefinite loading skeleton when navigation and silent polling overlap.
- Added regression coverage for slow folder responses, stale polling races,
  and re-entering a bucket after navigation.
- Reworked bucket-table geometry so modified timestamps, row action icons, and
  row bottoms stay aligned; widened the action column to prevent icon overlap
  and kept the layout usable on narrow screens.
- Added labeled Delete and Move icons to the bulk-selection toolbar and fixed
  nested-object deletion so it sends the complete key without duplicating the
  current folder prefix.
- Refined the operator shell with a polished account card and sign-out action,
  restored the Telegram connection badge on Recovery, and aligned the
  destructive-action checkbox with its label.
- Expanded Overview with storage composition, recovery signal, transfer
  pipeline, system checks, and storage-safeguard analytics.
- Added process-scoped client/Telegram payload counters for upload and download
  directions, exposed them through the overview API, visualized them separately
  on Overview, and refreshed the snapshot every five seconds. Counters exclude
  protocol overhead and reset when the server process restarts.
- Updated operator, roadmap, compatibility, storage-format, recovery, and
  frontend regression documentation for the shipped behavior.

## 0.7.3-rc.4 - 2026-09-26

Done jobs:

- Prevented a slow folder listing from being superseded by the silent browser
  activity poll, which could leave the bucket browser on an infinite loading
  skeleton despite successful API responses.
- Added a Playwright regression covering the slow-listing and polling race.
- Documented the admin-browser loading guarantee across the operator, storage,
  compatibility, and recovery guides.

## 0.7.3-rc.3 - 2026-09-25

Done jobs:

- Classified Telegram `FLOOD_WAIT` as an explicit pre-publication rejection:
  durable uploads now pause and resume automatically instead of incorrectly
  entering acknowledgement recovery. Existing persisted flood-wait rows are
  upgraded safely after any genuinely ambiguous send has been reconciled.
- Serialized a transfer's Telegram sends so a failed chunk cannot cancel a
  sibling send and leave its acknowledgement indeterminate.
- Coalesced repeated in-flight S3 `UploadPart` requests onto the original
  durable job and discarded only safe, never-published duplicates after the
  canonical part commits.
- Kept the file-browser's last successful multipart activity snapshot during a
  transient activity-poll failure, preventing in-flight rows from fading out
  and back in. Added a distinct automatic Telegram-pacing state.
- Added durable recovery, fault-matrix, and browser coverage for rate limiting,
  retry coalescing, and polling failure behavior.

## 0.7.3-rc.2 - 2026-09-25

Done jobs:

- Fixed large S3 multipart completion so uploaded Telegram chunks are composed
  into the final manifest instead of downloading and uploading the entire
  object a second time.
- Added manifest schema v2 payload provenance, cross-part range reads, and
  cleanup filtering that retains every chunk owned by the completed object.
- Safely superseded abandoned pre-composition completion receptions while
  preserving any ambiguous Telegram send for exact-byte reconciliation.
- Redesigned file-browser upload progress with accessible percentage bars and
  distinct receiving, uploading, finalizing, and needs-attention states.
- Added Rust restart/cleanup/composition regression coverage and Playwright
  coverage for in-flight, 269/269 finalization, and interrupted uploads.

## 0.7.3-rc.1 - 2026-09-25

Done jobs:

- Added active S3 multipart progress to the authenticated bucket browser.
- Added an uploading indicator with completed/total S3 part counts and a
  recovery-required state that keeps partial objects non-downloadable.
- Added the authenticated multipart-progress admin API and Playwright coverage.
- Preserved staged Telegram payloads until ambiguous acknowledgements are
  reconciled; interrupted uploads are not silently deleted.

## 0.7.2-rc.1 - 2026-09-11

Done jobs:

- Added a shared-link count badge and link-manager action to every file row in
  the admin bucket browser.
- Added a polished shared-links modal with descriptions, public URL display and
  copy, expiry editing, expired-link visibility, and revoke confirmation.
- Added authenticated list/update/revoke APIs and schema v12 migration for
  encrypted share-token ciphertext and descriptions while preserving hash-based
  public lookup.
- Added Rust metadata, admin API, frontend, and Playwright coverage for the
  complete share-link lifecycle.
- Updated operator, storage-format, migration, compatibility, and recovery
  documentation.

## 0.7.15 - 2026-09-11

- Promoted the validated `v0.7.15-rc.7` build to stable.
- Includes durable expiry and optional `/_public/` sharing, live database-backed
  chunk-size settings, polished upload/share workflows, guarded admin deletion,
  and aligned bucket-table actions.
- Completed Rust, frontend, and 24-test Playwright verification for the release.

## 0.7.15-rc.7 - 2026-09-11

- Fixed admin object deletion so success is reported only after the local
  tombstone commits; missing objects now return `404`.
- Guarded folder deletion against active child objects and pending transfers,
  returning a conflict instead of falsely reporting a non-empty folder deleted.
- Aligned bucket-table columns and row action icons, and added regression
  coverage for delete visibility, folder conflicts, and action alignment.

## 0.7.15-rc.2 - 2026-09-10

- Added background expiry sweeping so expired active objects are tombstoned
  and handed to the existing evidence-first cleanup worker.

## 0.7.15-rc.1 - 2026-09-10

- Added durable per-object expiry for S3, multipart, resumable admin, and
  browser uploads.
- Added authenticated admin-created bearer share links with optional expiry
  and bounded public GET/HEAD downloads.
- Added metadata schema migration support for hashed share-link records.

## 0.7.1 - 2026-09-10

- Promoted the Telegram account wizard and responsive admin console from the
  release-candidate series to the stable `v0.7.1` release.
- Verified the release with Rust and frontend checks, workspace tests, and
  responsive browser coverage from 320px through 1440px.

## 0.7.1-rc.12 - 2026-09-10

- Made the authenticated admin console responsive across phone, tablet, and
  desktop layouts, including touch-friendly navigation, collapsing forms, and
  bounded table scrolling.
- Added viewport-matrix browser coverage for 320px through 1440px layouts.

## 0.7.1-rc.8 - 2026-09-10

- Added durable Telegram send-attempt tokens and diagnostics. Ambiguous upload
  acknowledgements are automatically reconciled by exact document-token and
  encrypted-byte matching, with safe retry when a complete scan finds no
  matching document.

## 0.7.1-rc.7 - 2026-09-10

- Fixed connection-removal cleanup so the owning Telegram transport remains
  available until evidence-first remote deletion completes; added a regression
  test and documented the single-connection multi-user boundary.

## 0.7.1-rc.6 - 2026-09-10

- Isolated Telegram connection-removal cleanup by durable connection
  generation so a new login cannot operate on a previous account's cleanup
  targets.
- Detached local namespace state before remote cleanup completes, preserving
  recovery-required evidence without blocking re-login.
- Required end-to-end Telegram storage health before reporting login success,
  mapped `AUTH_KEY_UNREGISTERED` to reauthorization, and honored
  `TELEGRAM_SESSION_PATH` consistently.
- Added schema-v8 migration coverage, lifecycle documentation, ADR-0008, and
  the local-only server debugging runbook.

## 0.7.1-rc.5 - 2026-09-09

- Added contextual retry panels for failed overview, recovery, bucket, folder,
  operator, transfer, and destination-browser loads instead of showing false
  empty states or indefinite skeletons.
- Fixed admin file moves so the destination is committed before the source is
  deleted, with visible move progress and same-path validation.

## 0.7.1-rc.4 - 2026-09-09

- Reworked the Telegram setup wizard into an accessible modal with fresh,
  operator-owned flow IDs, safe cancellation, stale-flow protection, and
  retryable invalid confirmation codes.
- Added empty-bucket deletion, parent-folder navigation, refresh/navigation
  skeletons, improved sign-out feedback, and a quieter Recovery header.
- Added integration coverage for Telegram flow restart/retry behavior and
  Unicode bucket deletion.

## 0.7.1-rc.3 - 2026-09-09

- Added retryable error handling when a lazy-loaded admin view asset is
  unavailable, preventing Telegram settings from remaining on an infinite
  skeleton.
- Added static-asset regression coverage for hashed frontend chunks and
  missing JavaScript assets.

## 0.7.1-rc.2 - 2026-09-09

- Fixed production admin-console asset resolution so hashed JavaScript and CSS
  files load from the bundled assets directory instead of falling back to an
  unrelated file.

## 0.7.1-rc.1 - 2026-09-09

- Added history-routed admin navigation, inline SVG icons, bounded toast
  notifications, a folder-aware move browser, and separate Telegram connection
  and proxy tabs.
- Added reception-only resumable admin uploads with offset recovery,
  cancellation cleanup, and browser-side resume metadata. Sessions expire after
  the existing inactivity lease and do not survive a server restart.

## 0.7.0-rc.4 - 2026-09-09

- Split the admin console into focused panels and modal components while
  preserving the existing authenticated workflows.
- Lazy-loaded secondary views and modal flows to reduce the initial frontend
  JavaScript payload by about 20% raw and 17% gzip.
- Consolidated tiny Svelte runtime helper chunks in the Vite production build.

## 0.7.0-rc.2 - 2026-09-09

- Refined the operator console with icon-based navigation, centered menu items,
  loading skeletons, Telegram connection-check feedback, and storage analysis
  cards.
- Added modal workflows for bucket creation, uploads, operator creation, and
  Telegram credentials; proxy mode is now an explicit select control.
- Added bucket item selection, bulk deletion, and move-to-bucket/folder flow.
- Fixed encoded Unicode bucket names when deleting buckets and hid misleading
  zero-byte failed transfer rows.

## 0.7.0-rc.1 - 2026-09-08

- Added acknowledgeable recovery issues: each issue now carries a stable
  content fingerprint, and acknowledgements persist server-side in
  `app_settings` so they are shared by all operators and survive restarts.
- Slimmed the admin overview to a single corrupted-files count
  (`unacknowledged_count`), moving the per-issue details to the Recovery view
  where they render collapsed by default and can be acknowledged or restored.
- Added inline, non-blocking loading feedback across the console: a top
  progress bar, per-panel skeletons, and spinners in the refresh buttons. The
  background overview poll stays silent.
- Stopped navigating into a bucket immediately after creating it, so the
  operator stays on the bucket list.
- Styled the native `select` control to match the other inputs and finished the
  palette migration off the legacy teal accent.

## 0.5.2-rc.8 - 2026-09-06

- Fixed Telegram settings saves so malformed Telegram API IDs are rejected
  before persistence, preserving the last valid settings.
- Hardened the admin settings refresh path so transport/session failures return
  JSON warnings after persistence instead of dropping the HTTP connection and
  surfacing as 502.

## 0.5.2-rc.1 - 2026-09-04

- Kept bootstrap alive when recovery-required objects are present and surfaced
  recovery diagnostics in the admin overview.
- Added clickable recovery detail rows so operators can inspect missing or
  corrupted files directly from the web UI.

## 0.5.1 - 2026-09-04

- Added bounded admin content streaming with ranged downloads backed by the
  shared object-format reader.
- Extended the operator UI and Telegram onboarding flow so the auth and setup
  experience stays in-browser.
- Aligned storage, recovery, and compatibility docs with the current Phase 9
  and Phase 3 behavior.

## 0.5.0 - 2026-09-04

- Added legacy S3 `ListObjects` support on the main data plane, backed by the
  same ordered local manifest index and delimiter grouping as `ListObjectsV2`,
  to improve compatibility with older tooling such as Dokploy/rclone.

## 0.4.2 - 2026-09-03

- Switched release-facing versioning to the semver `0.4.2` across the Rust
  crate, admin frontend package metadata, and Docker image documentation.
- Removed SHA-based Docker image tag publishing so tagged releases publish
  versioned image tags instead of `sha-*` tags.
- Clarified the admin UI around Telegram authorization, operator accounts, and
  in-app bucket creation, and added admin bucket create/delete coverage.

- Phase 0 upstream analysis completed.
- Initial Telegram S3 documentation set added.
- Integration strategy and storage format ADRs drafted.
