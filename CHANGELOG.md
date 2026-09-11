# Changelog

## Unreleased

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
