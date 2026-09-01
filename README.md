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
- Phase 6 production hardening is complete: adapter-bound envelope encryption,
  real repair and garbage-collection workflows, and localhost-only health and
  metrics endpoints are now part of the shipped server behavior.
- Phase 7 Docker packaging is complete: the repo now ships a production
  container image, a single-service compose file, persistent volumes, and a
  container entrypoint that validates config before starting the server.
- Phase 8 authenticated operator frontend is complete: the Rust server now
  serves an authenticated `/_admin` SPA and JSON API with HTTP-only cookie
  sessions, bootstrap-secret login, overview cards, and onboarding checks.
- GitHub Actions can now publish the Docker image to GHCR on pushes to `main`
  and on version tags, so you can pull a ready-made image onto a server
  without rebuilding locally.
- RustFS upstream was inspected at `47a3f5ef0110ee5af04bbb761a8bb5ed99a9ce15`.
- Telegram Drive upstream was inspected at `77518a93fbc8a8242f38e23e486a2d87d3f82fb2`.
- The repo now includes a SQLite-backed metadata store, migrations, a
  chunk-aware object-format service, adapter-bound envelope encryption, a
  RustFS-backed S3 server seam, Docker packaging assets, and CLI smoke-test
  coverage around `config check`, `doctor`, `db`, `index`, `repair`,
  `gc --dry-run`, the transport bootstrap/status/logout flows, and standard S3
  CRUD/range flows.

Key documents:

- `docs/upstream-analysis.md`
- `docs/s3-compatibility.md`
- `docs/telegram-transport.md`
- `docs/telegram-storage-format.md`
- `docs/configuration.md`
- `docs/disaster-recovery.md`
- `docs/limitations.md`
- `docs/adr/0001-rustfs-integration-strategy.md`
- `docs/adr/0002-phase-6-hardening-boundaries.md`
- `docs/adr/0004-docker-packaging-and-bootstrap-boundary.md`
- `docs/metadata-store.md`

The project intentionally treats Telegram as a constrained remote object store,
not as an unlimited backup target. Local metadata, manifests, and recovery
tooling are required.

## Published Docker Image

After the workflow runs, the image is available from GHCR as:

```text
ghcr.io/<owner>/<repo>:latest
```

You can also pin a release tag such as `ghcr.io/<owner>/<repo>:v0.1.0` or a
SHA tag if you want a fixed image.

To run it on a server:

```bash
docker pull ghcr.io/<owner>/<repo>:latest
docker run -d \
  --name telegram-s3 \
  -p 9000:9000 \
  -e TELEGRAM_API_ID=... \
  -e TELEGRAM_API_HASH=... \
  -e TELEGRAM_PHONE_NUMBER=... \
  -e TELEGRAM_STORAGE_CHAT_ID=... \
  -e TELEGRAM_S3_MASTER_KEY=... \
  -e TELEGRAM_ADMIN_BOOTSTRAP_SECRET=... \
  -e RUSTFS_ACCESS_KEY=... \
  -e RUSTFS_SECRET_KEY=... \
  -v telegram-s3-metadata:/var/lib/telegram-s3/metadata \
  -v telegram-s3-data:/var/lib/telegram-s3/data \
  -v telegram-s3-session:/var/lib/telegram-s3/session \
  ghcr.io/<owner>/<repo>:latest
```

If you prefer Compose, replace the local `build:` block with the published
`image:` reference and keep the same environment variables and volumes.
