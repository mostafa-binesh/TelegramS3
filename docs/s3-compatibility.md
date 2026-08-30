# S3 Compatibility Matrix

Phase 3 has implemented the manifest/chunk object-format backend, and Phase 4
now wires the RustFS-backed S3 server through that layer for the CRUD slice.
The rows below track externally visible S3 API wiring; implemented entries are
available through `server`, and the standard S3 CRUD smoke test now passes.

| API operation | Status | Test coverage | Compatibility notes | Telegram-specific limitation | Planned phase |
| --- | --- | --- | --- | --- | --- |
| Create bucket | implemented | cargo test | Creates the local bucket row and exposes it through the S3 server | Telegram has no native bucket primitive | 4 |
| Delete empty bucket | implemented | cargo test | Refuses non-empty buckets and preserves recoverable state until cleanup | Cleanup is asynchronous | 4 |
| List buckets | implemented | cargo test | Local index is authoritative for bucket visibility | Remote reconstruction is slower | 4 |
| Head bucket | implemented | cargo test | Reflects bucket metadata from the local store | Telegram metadata is indirect | 4 |
| Put object | implemented | cargo test | Chunk upload plus manifest commit through the object-format service | 2 GiB Telegram file limit | 4 |
| Get object | implemented | cargo test | Streams from manifest and chunks with checksum verification | Requires chunk fetch and verification | 4 |
| Head object | implemented | cargo test | Returns committed metadata only | Manifest rebuild may be needed | 4 |
| Delete object | implemented | cargo test | Tombstones before cleanup | Physical delete is deferred | 4 |
| List objects v2 | implemented | cargo test | Uses the local index and manifest list for ordering | Remote reconciliation lag exists | 4 |
| Copy object | implemented | cargo check | Reuses the bounded object-format backend for source-to-destination copies | Copy is still local-first rather than remote-atomic | 5 |
| Byte-range GET | implemented | cargo test | Maps ranges to chunk spans | Requires chunk-aware verification | 4 |
| Multipart initiation | implemented | cargo check | Persists durable upload state in the local metadata store | Multipart state is local | 5 |
| Multipart part upload | implemented | cargo check | Stages part data and checksums | Each part must stay under Telegram limits | 5 |
| Multipart completion | implemented | cargo check | Commits manifest atomically after staged parts are verified | Completion must reconcile staged parts | 5 |
| Multipart abort | implemented | cargo check | Marks upload aborted and cleans up local state | Abort is local cleanup | 5 |
| Multipart listing | implemented | cargo check | Lists live multipart sessions from the local journal/metadata | Telegram does not expose upload sessions natively | 5 |
| Conditional requests | implemented | cargo test | GET/HEAD/PUT and copy/delete preconditions honor ETag and timestamp guards | Requires strong object-state checks | 5 |
| Object versioning | implemented | cargo check | Version IDs are surfaced from manifests and version listings | Telegram lacks built-in versions | 5 |
| Delete markers | implemented | cargo check | Tombstones are listed as delete markers and remain recoverable until cleanup | Must be modeled locally | 5 |
| Object tags | compatibility gap | none yet | Must persist in manifest/index | Captions are not enough | 5 |
| Checksums | implemented | cargo test | Chunk and whole-object checksums are enforced during upload, read, and reconciliation | Telegram alone is not enough | 5 |
| Presigned URLs | compatibility gap | none yet | Likely local capability URLs only | Telegram is not a URL signer | 5 |
| Server-side copy | implemented | cargo check | Copy uses the local object-format backend and manifest reuse | Telegram copy may not preserve metadata exactly | 5 |
| Lifecycle cleanup | planned | none yet | Needs background GC and tombstone reconciliation | Cleanup is deferred | 6 |
| Batch delete | compatibility gap | none yet | Can be translated to per-object tombstones | Telegram does not batch object deletes | 6 |
| Bucket policies | compatibility gap | none yet | Policy evaluation belongs above storage | Telegram is out of scope | 6 |
| Retention/object lock | compatibility gap | none yet | Requires additional metadata and enforcement | Telegram cannot enforce S3 locks | 6 |
| Event notifications | compatibility gap | none yet | Eventing is an upper layer concern | Telegram is not the notifier | 6 |
| Encryption | planned | none yet | Adapter-bound encryption is possible | Range semantics become more expensive | 6 |
| Quotas | compatibility gap | none yet | Can be tracked locally | Telegram storage quotas are external | 6 |
| Metrics/health | planned | none yet | Local service metrics are required | Telegram visibility is indirect | 6 |
