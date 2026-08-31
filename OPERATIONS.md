# Operations

## Configuration

See `docs/configuration.md` for the environment variable contract and runtime
settings reference.

## Startup

- Validate permissions on session, metadata, and key files.
- Refuse to start if secrets are missing or obviously unsafe.
- Initialize the local metadata store before opening the server.
- Ensure Telegram connectivity is available before accepting writes.

## Common Commands

- `telegram-s3 auth login`
- `telegram-s3 auth status`
- `telegram-s3 auth logout`
- `telegram-s3 server`
- `telegram-s3 doctor`
- `telegram-s3 config check`
- `telegram-s3 index rebuild`
- `telegram-s3 index verify`
- `telegram-s3 repair --dry-run`
- `telegram-s3 repair`
- `telegram-s3 gc --dry-run`
- `telegram-s3 gc`
- `telegram-s3 upstream status`
- `GET http://127.0.0.1:9001/healthz`
- `GET http://127.0.0.1:9001/metrics`

## Docker Deployment

- Build the image with `docker compose build`.
- Start the main service with `docker compose up -d`.
- Run interactive Telegram login with `docker compose run --rm -it telegram-s3 auth login`.
- Keep the deployment to the single `telegram-s3` service; bootstrap,
  config validation, and foreground serving all happen inside that container.
- Mount metadata, data, and session state through the named volumes defined in
  `docker-compose.yml`.
- The container publishes the S3 listener on host port `9000` and keeps the
  admin listener loopback-only inside the container.
- Check health from inside the container namespace with
  `docker compose exec telegram-s3 curl -fsS http://127.0.0.1:9001/healthz`.

## Backups and Recovery

- Back up the local metadata database.
- Preserve the Telegram session file and encryption keys separately.
- Verify that a manifest-only recovery can rebuild the index.
- Use `repair --dry-run` before `repair` when staged, recovery-required, or
  orphaned rows need reconciliation.
- Use `gc --dry-run` before `gc` to confirm only tombstoned data older than the
  retention threshold will be removed.
- If the server runs in Docker, back up the named metadata, data, and session
  volumes together so the compose deployment can be recreated without losing
  state.

## Upgrade and Rollback

- Migrate the local database before a new server version starts serving traffic.
- Keep old versions available until the new version passes health checks.
- Never rotate secrets as an incidental upgrade step.
- Verify both the S3 listener and the loopback admin listener after upgrade.
