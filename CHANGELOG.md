# Changelog

## Unreleased

## 0.7.5-rc.3 - 2026-09-27

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
