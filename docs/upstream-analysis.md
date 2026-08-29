# Upstream Analysis

## Repositories and Commits

- RustFS: `47a3f5ef0110ee5af04bbb761a8bb5ed99a9ce15`
- Telegram Drive: `77518a93fbc8a8242f38e23e486a2d87d3f82fb2`

## Answers to the Discovery Questions

### 1. Where does RustFS perform physical object persistence?

Primarily in `upstream/rustfs/crates/ecstore/src/disk/local.rs`, with the disk
contract defined in `upstream/rustfs/crates/ecstore/src/disk/mod.rs`. The
object and multipart use-cases are coordinated by `ECStore` in
`upstream/rustfs/crates/ecstore/src/store/mod.rs`, which delegates to the set
and disk layers.

### 2. Is there an existing stable storage trait that can support an external backend?

Yes. `upstream/rustfs/crates/storage-api/src/object.rs` defines `ObjectIO`,
`ObjectOperations`, `ListOperations`, `MultipartOperations`, and
`NamespaceLocking`, and `upstream/rustfs/crates/storage-api/src/bucket.rs`
defines `BucketOperations`. These are real contracts, but the concrete backend
still flows through ECStore and disk semantics.

### 3. Which RustFS operations assume local filesystem semantics?

Examples include volume and file operations on `DiskAPI` in
`upstream/rustfs/crates/ecstore/src/disk/mod.rs`, plus the local path handling
and durability code in `upstream/rustfs/crates/ecstore/src/disk/local.rs`.
Specific assumptions include `PathBuf`-based addressing, renames, directory
creation and removal, symlink-escape checks, file metadata reads, and fsync-like
commit behavior.

### 4. Which RustFS features are above the storage boundary, and which are inseparable from the current disk implementation?

Above the storage boundary:

- auth and request routing
- admin APIs and observability
- IAM and policy enforcement
- HTTP middleware and S3-visible request shaping

Inseparable from the current disk implementation:

- erasure coding
- disk and set topology
- local format and heal behavior
- multipart and object commit internals
- recovery of local metadata and repair paths

### 5. Can a Telegram adapter be implemented as an independent crate?

Not as a drop-in backend today. RustFS exposes useful contracts, but the actual
runtime is wired around ECStore, disk layout, and many disk-centric invariants.
An independent crate would still need a maintainable upstream seam.

### 6. If not, what is the smallest maintainable patch required in RustFS?

The least risky path is a maintained RustFS fork or an upstreamable storage
abstraction that lets the server keep its request processing while swapping the
physical object backend. For this project, the fork path is the smallest
maintainable fit.

### 7. Which parts of Telegram Drive's backend can be extracted without depending on Tauri, React, or desktop-only state?

Good extraction candidates:

- `app/src-tauri/src/crypto/envelope/*`
- `app/src-tauri/src/crypto/policy.rs`
- `app/src-tauri/src/crypto/registry.rs`
- `app/src-tauri/src/crypto/vault/*`
- `app/src-tauri/src/commands/auth.rs` session handling and login flow logic
- `app/src-tauri/src/vpn_optimizer.rs` network configuration logic
- `app/src-tauri/src/socks5_bridge.rs`
- `app/src-tauri/src/db.rs` and `app/src-tauri/src/db_migrations.rs`

The command modules themselves are still Tauri-wrapped, but much of the logic is
pure Rust and portable.

### 8. What Telegram library and session format does Telegram Drive currently use?

It uses `grammers-client`, `grammers-mtsender`, `grammers-session`, and
`grammers-tl-types`. The session is a SQLite session via
`grammers_session::storages::SqliteSession`, saved to `telegram.session`.

### 9. How are proxying, retries, flood waits, streaming uploads, and downloads currently implemented?

- Proxying: `NetworkConfig::effective_proxy_url()` builds a `socks5://...`
  URL. SOCKS5 is passed directly to grammers. HTTP/HTTPS proxies are bridged
  through a local SOCKS5 listener in `socks5_bridge.rs`.
- Retries and flood waits: `api_routes.rs` and `commands/fs.rs` use retry loops
  with configurable backoff, and flood waits sleep explicitly instead of
  silently failing over.
- Streaming uploads: files are uploaded with `client.upload_stream(...)` after a
  bounded staging step.
- Streaming downloads: chunks are read with `client.iter_download(...)`, then
  written to temporary files or decrypted incrementally, with bounded buffering.

### 10. What S3 behaviors cannot be represented directly by Telegram and require an index, transaction journal, or compatibility layer?

At minimum:

- atomic object overwrite visibility
- multipart commit atomicity
- versioning and delete markers
- strong listings over a mutable remote store
- tags and retention semantics
- conditional request enforcement
- consistent HEAD/GET/LIST behavior under crash recovery
- object-level checksums and manifest reconstruction

Those behaviors require a local journal, a manifest format, and recovery logic.

## License Notes

RustFS is Apache-2.0. Telegram Drive's checkout did not include a root
`LICENSE` file, so reuse must wait for a follow-up license confirmation pass.

