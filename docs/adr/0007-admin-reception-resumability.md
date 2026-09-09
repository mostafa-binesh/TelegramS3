# ADR-0007: Admin reception-only resumability

## Status

Accepted

## Decision

The authenticated admin browser upload path supports resumable reception in
bounded encrypted chunks. The server creates the existing `receiving`
`transfer_jobs` row, stages each chunk through the object-format encryption
path, renews the normal 120-second inactivity lease, and publishes the manifest
only after the client sends a final chunk. The browser re-queries the
authoritative offset after a failed request and never assumes that a lost HTTP
response means a chunk was not accepted.

Reception state is intentionally held in process memory. A server restart ends
the in-flight browser session; the durable job and staging/recovery machinery
remain authoritative for cleanup and operator diagnosis. This keeps the
feature limited to browser-to-server network interruptions and avoids inventing
a second persistent upload-session schema beside the existing journal.

Pause is a client-side stop between chunks. Resume is valid only while the
receiving lease remains active. Cancel removes the reception from the active
map, marks the job `reception_failed`, and removes its staging directory when
possible.

## Consequences

- Partial uploads remain invisible because no manifest is committed before
  completion.
- Chunk memory remains bounded by the configured chunk size.
- Browser reconnects can recover from duplicate or lost PATCH responses using
  the server offset.
- Restart recovery is explicit rather than falsely presented as browser-session
  persistence; operators must re-select the source file after expiry.

## Rejected alternatives

- Persisting browser session metadata in SQLite would expand this change into a
  second durable protocol and would require durable source-file identity and
  cleanup semantics.
- Treating this as S3 multipart upload would misrepresent the admin-plane
  contract and Telegram-backed transfer lifecycle.
