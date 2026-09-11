# ADR-0010: Object expiry and capability share links

## Status

Accepted

## Decision

Store an optional RFC3339 `expires_at` directly in every object manifest. The
object-format service filters expired active manifests at the authoritative
S3/admin lookup boundary, so expiry survives metadata-index rebuilds and is
not merely presentation metadata. Expired payloads remain subject to the
existing tombstone, evidence, retention, and garbage-collection workflow.

The authenticated admin panel can create an opaque random bearer token for a
committed object. The token hash remains the lookup authority, while the
encrypted token ciphertext is stored so the admin UI can manage existing links
after the create response has been closed. The raw token is returned as
`/_public/<token>`. A share link can have no independent deadline or an
optional deadline, but it is always capped by the object expiry. Public share
downloads use the same bounded, checksum-verifying reader as S3 GET and are
limited to GET/HEAD. The object browser exposes a per-file link count and a
management modal for descriptions, copying, expiry changes, and revocation.

Metadata schema version 12 adds the encrypted token ciphertext and optional
description columns. Existing hash-only rows remain valid for public lookup
and revocation, but their original URL cannot be reconstructed and the UI
labels them accordingly.

S3 uploads use the extension headers
`x-amz-meta-telegram-s3-expires-at` or
`x-amz-meta-telegram-s3-expires-in`. AWS SigV4 presigned URLs are not implied
by this feature and remain a separate compatibility gap.

## Rationale

Captions or browser state cannot rebuild access policy after local recovery.
Embedding expiry in the manifest keeps the policy alongside the data-plane
identity, while hashed tokens preserve constant-time lookup and encrypted
ciphertext enables authenticated link management without storing bearer
secrets in plaintext. A separate share deadline makes it possible to issue a
short-lived link for a long-lived object without changing object visibility.
