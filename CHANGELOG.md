# Changelog

## Unreleased

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
