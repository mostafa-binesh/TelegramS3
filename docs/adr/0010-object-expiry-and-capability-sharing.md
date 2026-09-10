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
committed object. Only the token hash is stored; the raw token is returned once
as `/share/<token>`. A share link can have no independent deadline or an
optional deadline, but it is always capped by the object expiry. Public share
downloads use the same bounded, checksum-verifying reader as S3 GET and are
limited to GET/HEAD.

S3 uploads use the extension headers
`x-amz-meta-telegram-s3-expires-at` or
`x-amz-meta-telegram-s3-expires-in`. AWS SigV4 presigned URLs are not implied
by this feature and remain a separate compatibility gap.

## Rationale

Captions or browser state cannot rebuild access policy after local recovery.
Embedding expiry in the manifest keeps the policy alongside the data-plane
identity, while local hashed tokens avoid placing reusable bearer secrets in
metadata backups. A separate share deadline makes it possible to issue a
short-lived link for a long-lived object without changing object visibility.
