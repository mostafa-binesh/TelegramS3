# ADR-0013: Resilient Telegram-Backed Downloads

## Status

Accepted.

## Context

An S3 or public-share download is a long-lived stream assembled from immutable
Telegram documents. The server must fetch, decrypt, and checksum-verify each
manifest chunk before sending its requested bytes to the client. A transient
Telegram or proxy failure during one remote read previously ended the HTTP
response immediately. Public clients also had no explicit `Accept-Ranges`
signal, even though the handler already supported single byte ranges.

## Decision

- Apply the configured bounded Telegram retry and flood-wait policy to
  transient RPC and transport I/O failures while reading the current chunk.
- Discard a partial remote fetch before retrying; emit no bytes from that chunk
  until the complete payload has passed the existing decrypt/checksum path.
- Do not retry missing messages, decryption failures, checksum mismatches, or
  other permanent object-integrity errors as network failures.
- Advertise `Accept-Ranges: bytes` on public-share responses and preserve the
  existing `Range`/`Content-Range` behavior so clients can resume after retries
  are exhausted.
- Log exhausted object-stream chunk failures with object, chunk, and Telegram
  message identifiers without changing object visibility or recovery state.

## Consequences

- Short Telegram/proxy interruptions no longer immediately break a client
  download.
- An exhausted stream can be resumed by a capable client without restarting
  the entire object.
- Retrying a chunk may repeat its Telegram download traffic, and flood-wait
  delays can extend the response duration. The retry count remains bounded by
  the existing transport policy.
- No metadata migration is required. Existing manifests and chunk references
  remain unchanged, and integrity failures still fail closed for recovery.
