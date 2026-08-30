# Telegram S3

Telegram S3 is a Telegram-backed object storage project that aims to keep the S3
surface compatible while persisting object data through private Telegram chats
and channels.

Current state:

- Phase 0 upstream analysis is complete.
- Phase 1 foundation is complete.
- Phase 2 Telegram transport now covers headless login, session reuse, proxy
  resolution, and a shared retry policy.
- Phase 3 object format now covers chunk planning, checksum verification, and
  journal-backed reconciliation.
- Phase 4 RustFS integration is complete: `server` now boots the RustFS-backed
  S3 listener and routes CRUD through the Phase 3 object-format service, and
  standard S3 smoke verification now passes.
- Phase 5 multipart and advanced compatibility is complete: multipart
  sessions, part uploads, completion, abort, version listings, and copy paths
  now flow through the same object-format backend, with checksum enforcement
  and reconciliation keeping the object model honest. Conditional request
  checks now apply to read, write, copy, and delete paths.
- RustFS upstream was inspected at `47a3f5ef0110ee5af04bbb761a8bb5ed99a9ce15`.
- Telegram Drive upstream was inspected at `77518a93fbc8a8242f38e23e486a2d87d3f82fb2`.
- The repo now includes a SQLite-backed metadata store, migrations, a
  chunk-aware object-format service, a RustFS-backed S3 server seam, and CLI
  smoke-test coverage around `config check`, `doctor`, `db`, `index`, the
  transport bootstrap/status/logout flows, and standard S3 CRUD/range flows.

Key documents:

- `docs/upstream-analysis.md`
- `docs/s3-compatibility.md`
- `docs/telegram-transport.md`
- `docs/telegram-storage-format.md`
- `docs/configuration.md`
- `docs/disaster-recovery.md`
- `docs/limitations.md`
- `docs/adr/0001-rustfs-integration-strategy.md`
- `docs/metadata-store.md`

The project intentionally treats Telegram as a constrained remote object store,
not as an unlimited backup target. Local metadata, manifests, and recovery
tooling are required.
