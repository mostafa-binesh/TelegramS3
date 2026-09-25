# ADR-0011: Compose Multipart Manifests Without Payload Re-upload

## Status

Accepted.

## Context

Multipart parts are already encrypted, checksum-verified, and durable Telegram
documents. The original completion path downloaded every selected part,
decrypted it, staged the concatenated object, encrypted it under a new identity,
and uploaded all bytes again before publishing the final manifest. For objects
larger than 1 GiB this kept `CompleteMultipartUpload` open for minutes, could
outlive the client request, doubled Telegram traffic, and left the operator UI
at all parts complete without a committed object.

The existing encryption envelope derives a chunk key, nonce, and authenticated
data from the source object UUID and chunk order. Reusing a Telegram document
therefore requires preserving that identity explicitly. Cleanup also cannot
delete part chunks after they become references of the final object.

## Decision

- Manifest schema v2 adds paired optional `source_object_id` and
  `source_chunk_order` fields to each chunk reference.
- Multipart completion validates ordered part metadata and flattens the part
  manifests into one contiguous final chunk map. Each final chunk retains its
  Telegram location, plaintext checksum, and effective source payload identity.
- The completion transfer has no payload chunks to publish. It durably stages
  and uploads only the final manifest, then atomically commits object visibility
  and the multipart session transition.
- Ordinary PUTs retain raw whole-object `sha256`. Multipart manifests use the
  domain-separated `sha256-parts-v1` composite checksum over ordered part
  number, size, checksum algorithm, and verified part checksum.
- Completion cleanup queues private part manifests but excludes every Telegram
  chunk location retained by the final object.
- Old abandoned completion jobs may be superseded only when they have no
  ambiguous send attempt. `sending` or `unknown` evidence still requires the
  existing token and exact-encrypted-byte reconciliation.

## Consequences

- Completion time and local staging use are proportional to manifest size, not
  object size; a 269-part backup no longer performs a second 1+ GiB transfer.
- Full and range reads can span part boundaries while remaining bounded to one
  chunk and verifying every plaintext chunk checksum.
- Final manifests are sufficient for index rebuild because they contain every
  Telegram location and decryption identity.
- Schema v1 remains readable. Older binaries do not understand schema v2 source
  identities and must not be used to read newly composed objects.
- The multipart ETag/checksum is a deterministic composite identity and is not
  advertised as the raw SHA-256 of concatenated bytes.
