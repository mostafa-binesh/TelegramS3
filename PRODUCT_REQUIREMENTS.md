# Product Requirements

## MVP

- Create bucket
- Delete empty bucket
- List buckets
- Head bucket
- Put object
- Get object
- Head object
- Delete object
- List objects v2
- Copy object
- Byte-range GET
- Multipart initiation
- Multipart part upload
- Multipart completion
- Multipart abort
- Multipart recovery after restart
- User metadata
- Content type
- Clear ETag semantics

## Production Requirements

- Durable local metadata database
- Operation journal and recovery
- Direct and SOCKS5 proxy support
- Retry and flood-wait handling
- Bounded memory use for uploads and downloads
- Garbage collection and repair commands
- Metrics and health endpoints
- Secret redaction and permission checks
- Documented upgrade and rollback procedure

## Optional Advanced Features

- Conditional requests
- Object versioning
- Delete markers
- Object tags
- Checksums beyond ETag
- Presigned URLs
- Server-side copy
- Lifecycle cleanup
- Multipart listing
- Batch delete
- Bucket policies
- Retention / object lock
- Event notifications
- Encryption
- Quotas

## Explicit Non-goals

- Treating Telegram as a single source of truth without a local journal
- Buffering full large objects in memory
- Silent fallback to insecure network paths
- Pretending unsupported S3 operations are fully implemented
- Requiring interactive Telegram login on every restart

## Acceptance Criteria

- A standard S3 client can exercise the MVP path.
- Interrupted writes do not expose partial objects.
- Restart reconciliation is tested.
- Telegram manifests can rebuild the index.
- Proxy configuration works without leaking secrets.
- Docs match the observed behavior.

## Supported and Unsupported S3 APIs

The compatibility matrix in `docs/s3-compatibility.md` is authoritative.

