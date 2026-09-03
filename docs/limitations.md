# Limitations

## Telegram Limits

- Telegram documents are capped at roughly 2 GiB.
- Rate limits and flood waits are external and must be respected.
- Connectivity may require a proxy in some environments.
- Encryption is adapter-bound and keyed locally; there is no separate key
  management service.

## S3 Semantics

- Strong transactional semantics are not available directly from Telegram.
- Multipart operations need a local journal.
- Versioning, delete markers, tags, and object lock need compatibility layers.

## Operational Limits

- The local database is part of the durability model.
- Recovery is only as good as the manifests and backups kept.
- The project must not be described as an unlimited or sole backup target.
- Health and metrics remain on the loopback admin listener, while the
  authenticated operator frontend and JSON API are served from the public
  listener under `/_admin`.
- Operator accounts live in `metadata.sqlite`; losing it means re-provisioning
  the first superuser with `telegram-s3 users create`. Password hashes are
  not recoverable (argon2id + per-account salt) and there is no password-reset
  email — recovery is CLI-admin only.
- Every operator is admin-tier for now; the stored `role` is reserved for future
  per-user/tenant scopes. All authenticated operators currently see the full
  bucket/object surface.
- The `/_admin` operator surface exposes a bucket/object browser (prefix folders
  + directory markers + delete) and **bounded binary upload/download** whose
  transport flows through the S3 data plane (see `docs/s3-compatibility.md`).
  Upload/download memory stays bounded per chunk; the Telegram **login wizard**
  drives the real single-account login. Guest access, and browsers without a
  valid session, get `401`/`403` on the content and wizard APIs.
- Explicit future items (not claimed as done): bulk **folder download** /
  server-side ZIP (would need whole-object buffering) and drag-in of nested
  directory trees; real Telegram **byte transport** (the object store currently
  reads/writes committed local chunk files and reports `telegram_document_id` as
  `local:`).
