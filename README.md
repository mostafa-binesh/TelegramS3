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
  sessions, overview cards, and onboarding checks.
- Phase 9 multi-user control plane is now in progress with the bounded-content
  streaming and Telegram-wizard increment landed: operator accounts are
  argon2id-hashed in `metadata.sqlite` (schema `4`) and managed in-app or via
  the `telegram-s3 users` CLI (no per-user `.env`); sessions are bound to a user
  and revocable; login is rate-limited; and guests only ever see a sign-in
  screen. The browser UI covers overview, operator management, a bucket/object
  browser, per-file bounded **upload/download** (full + range), and an
  in-browser **Telegram onboarding wizard**. See
  `docs/adr/0006-multiuser-control-plane.md`.
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
- `docs/adr/0006-multiuser-control-plane.md`
- `docs/metadata-store.md`

The project intentionally treats Telegram as a constrained remote object store,
not as an unlimited backup target. Local metadata, manifests, and recovery
tooling are required.

## Published Docker Image

After the workflow runs, the image is available from GHCR as:

```text
ghcr.io/<owner>/<repo>:latest
```

You can also pin a release tag such as `ghcr.io/<owner>/<repo>:v0.4.0` or a
SHA tag if you want a fixed image.

For releases, tag the repo with a semantic version like `v0.4.0`. The workflow
will publish the tag itself plus semver-friendly image aliases, and it prints
the final image tags at the end of the GitHub Actions run.

To run it on a server with Docker Compose:

```yaml
services:
  telegram-s3:
    image: ghcr.io/<owner>/<repo>:latest
    init: true
    restart: unless-stopped
    environment:
      TELEGRAM_API_ID: ${TELEGRAM_API_ID}
      TELEGRAM_API_HASH: ${TELEGRAM_API_HASH}
      TELEGRAM_PHONE_NUMBER: ${TELEGRAM_PHONE_NUMBER}
      TELEGRAM_STORAGE_CHAT_ID: ${TELEGRAM_STORAGE_CHAT_ID}
      TELEGRAM_METADATA_PATH: /var/lib/telegram-s3/metadata/metadata.sqlite
      TELEGRAM_DATA_DIR: /var/lib/telegram-s3/data
      TELEGRAM_SESSION_PATH: /var/lib/telegram-s3/session/telegram.session
      TELEGRAM_S3_BIND_ADDR: 0.0.0.0:9000
      TELEGRAM_ADMIN_BIND_ADDR: 127.0.0.1:9001
      TELEGRAM_S3_MASTER_KEY: ${TELEGRAM_S3_MASTER_KEY}
      TELEGRAM_ADMIN_BOOTSTRAP_SECRET: ${TELEGRAM_ADMIN_BOOTSTRAP_SECRET}
      RUSTFS_ACCESS_KEY: ${RUSTFS_ACCESS_KEY}
      RUSTFS_SECRET_KEY: ${RUSTFS_SECRET_KEY}
      TELEGRAM_PROXY_MODE: ${TELEGRAM_PROXY_MODE:-auto}
      TELEGRAM_PROXY_URL: ${TELEGRAM_PROXY_URL:-}
      TELEGRAM_PROXY_USERNAME: ${TELEGRAM_PROXY_USERNAME:-}
      TELEGRAM_PROXY_PASSWORD: ${TELEGRAM_PROXY_PASSWORD:-}
      TELEGRAM_CHUNK_SIZE: ${TELEGRAM_CHUNK_SIZE:-}
      TELEGRAM_CONNECTION_TIMEOUT_SECS: ${TELEGRAM_CONNECTION_TIMEOUT_SECS:-}
      TELEGRAM_REQUEST_TIMEOUT_SECS: ${TELEGRAM_REQUEST_TIMEOUT_SECS:-}
      TELEGRAM_TRANSFER_TIMEOUT_SECS: ${TELEGRAM_TRANSFER_TIMEOUT_SECS:-}
      TELEGRAM_RETRY_COUNT: ${TELEGRAM_RETRY_COUNT:-}
      TELEGRAM_RETRY_BACKOFF_MS: ${TELEGRAM_RETRY_BACKOFF_MS:-}
      TELEGRAM_FLOOD_WAIT_RESPECT: ${TELEGRAM_FLOOD_WAIT_RESPECT:-true}
    ports:
      - "9000:9000"
    volumes:
      - telegram-s3-metadata:/var/lib/telegram-s3/metadata
      - telegram-s3-data:/var/lib/telegram-s3/data
      - telegram-s3-session:/var/lib/telegram-s3/session
    healthcheck:
      test: ["CMD", "curl", "-fsS", "http://127.0.0.1:9001/healthz"]
      interval: 30s
      timeout: 5s
      start_period: 20s
      retries: 3

volumes:
  telegram-s3-metadata:
  telegram-s3-data:
  telegram-s3-session:
```

After saving that as something like `docker-compose.prod.yml`, run:

```bash
docker compose -f docker-compose.prod.yml up -d
```

If you publish a release and want to pin it, replace `latest` with the version
tag, for example `ghcr.io/<owner>/<repo>:v0.4.0`.
