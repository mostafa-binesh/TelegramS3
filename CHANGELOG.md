# Changelog

## Unreleased

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
