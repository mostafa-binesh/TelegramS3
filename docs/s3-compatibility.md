# S3 Compatibility Matrix

Phase 3 has implemented the manifest/chunk object-format backend, but the
rows below still track externally visible S3 API wiring. Those APIs remain
planned until the RustFS integration phase exposes them through the server.

| API operation | Status | Test coverage | Compatibility notes | Telegram-specific limitation | Planned phase |
| --- | --- | --- | --- | --- | --- |
| Create bucket | planned | none yet | Must create remote namespace and local bucket row | Telegram has no native bucket primitive | 4 |
| Delete empty bucket | planned | none yet | Must refuse non-empty buckets | Cleanup is asynchronous | 4 |
| List buckets | planned | none yet | Local index should be authoritative | Remote reconstruction is slower | 4 |
| Head bucket | planned | none yet | Must reflect bucket metadata | Telegram metadata is indirect | 4 |
| Put object | planned | none yet | Chunk upload + manifest commit | 2 GiB Telegram file limit | 4 |
| Get object | planned | none yet | Stream from manifest and chunks | Requires chunk fetch and verification | 4 |
| Head object | planned | none yet | Return committed metadata only | Manifest rebuild may be needed | 4 |
| Delete object | planned | none yet | Tombstone before cleanup | Physical delete is deferred | 4 |
| List objects v2 | planned | none yet | Use local index first | Remote reconciliation lag exists | 4 |
| Copy object | planned | none yet | Prefer metadata-aware copy path | Telegram copy semantics are not atomic with local commit | 5 |
| Byte-range GET | planned | none yet | Map ranges to chunk spans | Requires chunk-aware verification | 4 |
| Multipart initiation | planned | none yet | Persist durable upload state | Multipart state is local | 5 |
| Multipart part upload | planned | none yet | Stage part and checksum | Each part must stay under Telegram limits | 5 |
| Multipart completion | planned | none yet | Commit manifest atomically | Completion must reconcile staged parts | 5 |
| Multipart abort | planned | none yet | Mark upload aborted and clean up | Abort is eventually consistent | 5 |
| Multipart listing | planned | none yet | Can be backed by local journal | Telegram does not expose upload sessions natively | 5 |
| Conditional requests | compatibility gap | none yet | Must be enforced in the adapter | Requires strong object-state checks | 5 |
| Object versioning | compatibility gap | none yet | Needs version IDs in manifest/index | Telegram lacks built-in versions | 5 |
| Delete markers | compatibility gap | none yet | Needs tombstones | Must be modeled locally | 5 |
| Object tags | compatibility gap | none yet | Must persist in manifest/index | Captions are not enough | 5 |
| Checksums | planned | none yet | Manifest should carry per-chunk and whole-object checksums | Telegram alone is not enough | 5 |
| Presigned URLs | compatibility gap | none yet | Likely local capability URLs only | Telegram is not a URL signer | 5 |
| Server-side copy | compatibility gap | none yet | Copy should prefer local manifest reuse | Telegram copy may not preserve metadata exactly | 5 |
| Lifecycle cleanup | planned | none yet | Needs background GC and tombstone reconciliation | Cleanup is deferred | 6 |
| Batch delete | compatibility gap | none yet | Can be translated to per-object tombstones | Telegram does not batch object deletes | 6 |
| Bucket policies | compatibility gap | none yet | Policy evaluation belongs above storage | Telegram is out of scope | 6 |
| Retention/object lock | compatibility gap | none yet | Requires additional metadata and enforcement | Telegram cannot enforce S3 locks | 6 |
| Event notifications | compatibility gap | none yet | Eventing is an upper layer concern | Telegram is not the notifier | 6 |
| Encryption | planned | none yet | Adapter-bound encryption is possible | Range semantics become more expensive | 6 |
| Quotas | compatibility gap | none yet | Can be tracked locally | Telegram storage quotas are external | 6 |
| Metrics/health | planned | none yet | Local service metrics are required | Telegram visibility is indirect | 6 |
