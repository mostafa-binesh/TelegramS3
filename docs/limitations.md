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
