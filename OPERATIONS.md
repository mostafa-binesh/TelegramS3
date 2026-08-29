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
- `telegram-s3 repair`
- `telegram-s3 gc --dry-run`
- `telegram-s3 gc`
- `telegram-s3 upstream status`

## Backups and Recovery

- Back up the local metadata database.
- Preserve the Telegram session file and encryption keys separately.
- Verify that a manifest-only recovery can rebuild the index.

## Upgrade and Rollback

- Migrate the local database before a new server version starts serving traffic.
- Keep old versions available until the new version passes health checks.
- Never rotate secrets as an incidental upgrade step.

