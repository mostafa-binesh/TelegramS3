# Security Policy

## Handling Secrets

- Never commit Telegram API credentials, session files, recovery codes, S3 keys,
  or encryption keys.
- Use environment-variable placeholders only.
- Prefer operating-system secret storage when a secret must live beyond process memory.

## Logging Rules

- Redact phone numbers, proxy passwords, session paths, and message contents.
- Do not print login codes or cloud-password values.
- Keep structured logs free of full object keys or Telegram identifiers when avoidable.

## Telegram Authentication

- Interactive login must happen through a dedicated auth command, not inside the server startup path.
- Session persistence must survive restarts without re-prompting the user.

## Files and Permissions

- Session and key material should be stored with restrictive filesystem permissions.
- A startup check should reject unsafe permissions when feasible.

## Testing Boundaries

- Real Telegram smoke tests must be disabled by default.
- Never run destructive Telegram tests without explicit user confirmation and a dedicated test channel.

## Vulnerability Reporting

If you find a security issue, report it privately and avoid public disclosure until a fix is available.

