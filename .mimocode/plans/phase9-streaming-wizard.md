# Plan: Phase 9 next slice — bounded file streaming over `/_admin` + in-browser Telegram onboarding wizard

**Status:** open (deferred follow-up to the landed Phase 9 multi-user control-plane core).
**Opener instructions (for a new session):** This file is the full hand-off spec. Read the
referenced files/seams below, then implement each section. Keep the same green bar the core
established: `cargo fmt --all -- --check`, `cargo clippy --workspace --all-targets --all-features -- -D warnings`,
`cargo test --workspace --all-features`, and (frontend) `svelte-check` + `vite build`. Update docs + ADR-0006 +
ROADMAP Phase 9 as part of finishing (the honest-documentation rule in AGENTS.md).

---

## 0. What already exists (context you will build on — read these)

- Landing slice (done & green): credential multi-user control plane. See the plan it came from:
  `.mimocode/plans/1788441564257-eager-forest.md` (§1–§9 now implemented). Summary that matters here:
  - Accounts/sessions live in `metadata.sqlite` schema v4; auth in `src/auth.rs`;
    control-plane routes live in `src/admin.rs` (`AdminUiState`), all behind per-user signed-cookie
    sessions + CSRF.
  - `AdminUiState.handle_api` routes `/_admin/api/*`. File JSON surface already added:
    `GET /buckets`, `GET /objects?bucket=&prefix=&delimiter`, `POST /objects/folder`,
    `POST /objects/delete`.
  - Frontend `frontend/src/App.svelte` + `api.ts` + `types.ts` (Svelte5+Vite, base `/_admin`). The UI
    hints "Content upload/download arrives in the next slice."

- AGENTS.md invariants that bind this work: **never buffer a full object in RAM**; never hide
  unsupported/security behavior behind a success response; keep `/_admin` (control plane) distinct
  from S3 semantics; update the documented files when behavior changes.

## 1. Goals and non-goals for this slice

### Goals
1. **Bounded binary upload** from the browser into the existing S3 data plane.
2. **Bounded binary download** (single file, full + `Range`, `206`/`Content-Range`) to the browser.
3. **In-browser Telegram onboarding wizard** (credential gate is closed) that drives the real
   single-account Telegram login: phone → code → (only if Telegram requires it) 2FA/cloud password.

### Non-goals (explicit, do not silently expand)
- Real Telegram **byte transport** (the data plane stores/reads committed **local chunk files**;
  `telegram_document_id` is `local:`). Moving object bytes to/from Telegram is a separate structural
  task and is NOT in scope here.
- Bulk **folder** download / ZIP streaming; drag-in of nested directories. Document these as
  future, do not half-implement.
- Browser multipart-upload protocol negotiation / server-side resumable-multipart receive. Upload is a
  single streamed file (≤ the 2 GiB Telegram doc cap the S3 path already enforces via chunking).
- Account switching / signing in multiple Telegram accounts (server holds ONE Telegram session).

---

## 2. Part A — bounded upload (browser → store)

### 2.1 Data path (already exists, reuse)
`ObjectFormatService::put_stream(bucket, key, content_type, body: Option<StreamingBlob>)`
(`src/object_format.rs:833`) chunks an inbound stream by the configured chunk size, computes
chunk + whole-object checksums, writes encrypted chunk files, and commits a manifest. It is already
called by the S3 `put_object` handler (`src/s3_server.rs:~407`). So Part A upload is a **transport
bridge only**: turn `hyper::body::Incoming` into a `StreamingBlob` and hand it to `put_stream`.
Memory stays bounded because `put_stream` reads chunk-by-chunk internally.

### 2.2 Endpoint
- `POST /_admin/api/objects/content?bucket=<bucket>&key=<encoded full key>`
- Auth: authenticated session (existing principal path in `src/admin.rs`); mutating ⇒ CSRF required
  (consistent with the other writes).
- Body = the raw file bytes. The SPA sends the `File` via `fetch(..., { method:'POST', body: file })`
  with `Content-Type` set from the file (or `application/octet-stream`). No `multipart/form-data`
  server parser is needed (the frontend always runs JS).
- Content-Type: read from the request `Content-Type` header; fall back to `application/octet-stream`
  and derive a filename/extension guess only for `Content-Disposition` on download, never for storage.

### 2.3 Body adapter (the concrete seam)
`s3s::dto::StreamingBlob` (`…/s3s/dto/streaming_blob.rs`) exposes:
- `StreamingBlob::wrap<S,E>(stream: S)` where `S: Stream<Item=Result<Bytes,E>> + Send+Sync+'static`,
  `E: std::error::Error`.
- ByteStreams are already how `s3s` flows request bodies.

Implement a small adapter in `src/admin.rs` (or a shared `src/http_stream.rs`):
```rust
fn body_to_streaming_blob(req_body: hyper::body::Incoming) -> StreamingBlob {
    use http_body_util::BodyExt; // project already depends on http-body-util
    StreamingBlob::wrap(req_body.into_data_stream())  // Item = Result<Bytes, hyper::Error>
}
```
`hyper::Error: std::error::Error` so the bounds are satisfied. Response on success:
`200 { "size": u64, "etag": string, "version_id": string }` from the returned manifest
(`manifest.content_length`, `manifest.checksum.whole_object`, `manifest.object_id`).

### 2.4 Key validation (mirror S3 constraints)
Reject empty/absolute/slash-leading keys and `..` traversal (reuse/`extract` the same checks the
folder/delete handlers already use). Keys may be a full file path `dir/sub/name.ext`; the frontend
composes it from the open bucket + folder prefix + filename.

---

## 3. Part B — bounded download (store → browser), full + range

### 3.1 Factor the S3 fan-out into one shared bounded reader (recommended)
The S3 `get_object` already streams bounded, chunk-at-a-time, verifying checksum + decryption on the
fly: single-chunk fast path and the multi-span `stream::unfold` body at `src/s3_server.rs:815-925`.
That logic uses public primitives on `ObjectFormatService`: `plan_read` (assoc fn, ~1188),
`chunk_path`/`chunk_dir`, `decrypt_chunk` (`pub(crate)`), and `sha256_hex`.

Prefer to add ONE shared helper on `ObjectFormatService`, e.g.
```rust
pub(crate) fn read_spans_to_stream(
    &self,
    manifest: &ObjectManifest,
    spans: Vec<ReadSpan>,          // plan_read(manifest, range).chunks
) -> impl Stream<Item = Result<Bytes, std::io::Error>> + Send + Sync + '_
```
that emits one decrypted+verified slice per span (bounded by chunk size), then have BOTH the S3
`get_object` body and the admin download call it. This DRYs the streaming read and keeps S3 behavior
byte-for-byte identical. If factoring the S3 handler feels risky, an acceptable fallback is to
replicate the unfold inside the admin handler over the same `ObjectFormatService` calls — but never
call `read_bytes` (whole-`Vec`) or `read_range_to_writer` (whole-range sync materialize) on the
download path.

### 3.2 Endpoints
- `GET` and `HEAD /_admin/api/objects/content?bucket=<b>&key=<k>` (+ optional `Range: bytes=…`)
- Auth: authenticated session (safe methods: no CSRF needed).

Headers on response:
- `Content-Length` = span length (or full length)
- `Content-Type` = manifest content_type
- `ETag` = manifest checksum.whole_object
- `Content-Disposition: attachment; filename="<basename>"` — basename of the key, quoted safely
  (RFC 5987 percent-encode non-ASCII if present).
- Range request → status `206`, `Content-Range: bytes s-e/total`.
- HEAD returns headers only, no body.

### 3.3 Body seam (outbound)
`s3s::http::Body` (re-exported as `s3s::Body`) can be built from any `http_body::Body` via
`Body::http_body(..)` and streams via its `DynByteStream`. Build the admin response as
`http_body_util::combinators::BoxBody<Bytes, _>` / `StreamBody<impl Stream<Item=Result<Frame<_>,E>>>`
wrapping our `Result<Bytes, io::Error>` chunk stream (map the per-item error to the body error type),
then hand it to `Response::new(HttpBodyRemapped)`. Confirm exact error-type mapping against
`s3s::error::StdError` when writing. Do not convert the whole stream to a `Vec` at any point.

### 3.4 Folder download behaviour (decide + document)
S3 has no folder object unless a zero-byte `dir/` marker exists. Downloading a folder path with no
marker must return `404`; there is **no server zip** in this slice (avoid whole-RAM buffering). Record
"bulk folder download" as an explicit future item.

---

## 4. Part C — in-browser Telegram onboarding wizard

### 4.1 Current blocker to read first
`src/telegram/transport.rs` `interactive_login()` (lines ~303–378) is ONE synchronous, stdin-bound
`tty prompt()` flow that drives `client.request_login_code(phone)` → `sign_in(token, code)` and, on
`Err(SignInError::PasswordRequired(password_token))`, calls `client.check_password(token, pw)`
inline. There is no keyed/staged API a browser can resume. Single Telegram account/session is a
hard constraint (only ONE `SqliteSession` file + one `Client`).

### 4.2 Keep the Telegram client alive for the wizard
`AdminUiState` currently only holds a `transport_status` snapshot, not a live client. To drive a
wizard the server must keep a live `grammers Client`/`TelegramTransport` reachable. `S3Server::bootstrap`
creates `TelegramTransport` then discards everything but a status snapshot (`src/s3_server.rs:117-169`;
`src/admin.rs:31-58`).

Design intent: hold a single `Arc<TelegramTransport>` (or a small wrapper) in `AdminUiState`, created at
`bootstrap`, so the admin HTTP layer can call login primitives. Only construct it lazily when a wizard
is started (avoids paying for a kept-alive client when Telegram is already authorized and no wizard is
running). The transport must stay alive across a wizard's phone→code→password turns.

### 4.3 New driver — `src/telegram/login_driver.rs`
```rust
pub enum TgLoginPhase { Idle, Phone, Code, TwoFa, Authorized }
pub struct TelegramLoginDriver { /* step + per-attempt context, single-account mutex */ }

impl TelegramLoginDriver {
    pub async fn begin(&self, transport: &Arc<TelegramTransport>, phone: Option<String>) -> Result<TgLoginPhase, TgError>;
    pub async fn submit_code(&self, transport, code: &str) -> Result<TgLoginProgress, TgError>;  // Ok(progress) carries "needs_2fa" or authorized
    pub async fn submit_password(&self, transport, password: &str) -> Result<TgLoginPhase, TgError>;
    pub fn cancel(&self);
    pub fn state(&self) -> TgLoginPhase;
}
```
Rules:
- Exactly ONE wizard may be in flight per server process. A second authenticated operator calling
  `begin` while another is mid-flow → `409 { phase, owner_hint }`, never "both admins thrash tokens".
  (Simple: one global optional driver guarded by `Arc<Mutex<Option<_>>>`.)
- On `submit_code`, map grammers `SignInError::PasswordRequired(password_token)` to an
  "awaiting 2FA" progress and **retain the token** so `submit_password` calls
  `client.check_password(token, password)`.
- Reuse existing retry/flood-wait handling (`transport.rs`), and route failures through the existing
  `LoginFailure` classification types so the CLI and HTTP behave the same.
- Transport mock mode (`TELEGRAM_TRANSPORT_RUNTIME=mock`) should short-circuit to Authorized (tests).

### 4.4 Endpoints (authenticated session + CSRF on mutating)
- `GET  /_admin/api/telegram/wizard/state` → `{ phase }` (and `authorized` bool)
- `POST /_admin/api/telegram/wizard/begin`       body `{ "phone": optional }`
- `POST /_admin/api/telegram/wizard/submit-code` body `{ "code": "…" }`       → `{ progress }`
- `POST /_admin/api/telegram/wizard/submit-password` body `{ "password": "…" }`
- `POST /_admin/api/telegram/wizard/cancel`
Wizard is reachable/submitted ONLY by an authenticated operator — credentials (code/2FA) never cross
an unauthenticated page.

### 4.5 UI/UX (do it well — this is the point of the in-browser flow)
- Entry: navbar shows "Set up Telegram login" only when the signed-in operator sees the account is not
  yet authorized. Do NOT show it when Telegram sessions are already Authorized/Reused.
- Three clean, sequential steps, described in plain language (never jargon):
  1. **Phone** — pre-fill from config if present, editable. Explain this is the Telegram account whose
     empty/private storage chat this store writes to.
  2. **Code** — masked field, "resend" affordance + soft countdown when the transport supports it;
     friendly hint ("6-digit code sent to your Telegram app/phone").
  3. **2FA / cloud password** — shown ONLY when step 2 answered `PasswordRequired` (never shown
     preemptively; avoids confusing accounts without Two-Step). Masked; label it a "password / cloud
     password" and note it is sent once over the authenticated TLS session to finish sign-in.
  Each step: back/cancel/restart, inline error text, busy disable, and a friendlier server error.
- On completion refresh `/_admin/api/overview` so the Telegram-readiness panel flips to authorized.
- No step blames TLS/network: any transport error surfaces verbatim from the existing classification.

### 4.6 CLI regression
`telegram-s3 auth login` must keep working (headless/CI). Refactor it to drive the SAME driver,
replacing the HTTP inputs with stdout prompts + TTY reads. Do not delete `interactive_login`-style
behaviour without an equivalent; verify `tests/transport_smoke.rs` + any login CLI test still pass.

---

## 5. Frontend additions (Svelte5 + Vite, base `/_admin`) — `frontend/src/`
- `lib/api.ts` + `lib/types.ts`:
  - `uploadFile(csrf, fetchTarget)` using XHR or fetch with progress event → per-file progress;
  - `downloadUrl(bucket,key)` returns the `/_admin/api/objects/content?...` absolute URL used as an
    `<a download>` for the browser (cookie-authed automatically); HEAD for metadata if needed;
  - wizard calls: `getWizardState`, `beginTelegramWizard/phone`, `submitCode`, `submitPassword`, `cancel`.
- `App.svelte` file-browser view:
  - per open folder, an **Upload** affordance (file input + optional drag-drop target overlay),
    queue, per-file progress bar, then reload listing on completion.
  - each object row gets a **Download** action (link/button) and preserves the existing Delete action.
- A small **TelegramWizard** section component (steps as above). New ephemeral toasts for upload errors.

Design/seam notes already verified this session: `Request<Incoming>` is not `Clone` — read auth from
`principal_from_headers(&HeaderMap)` and keep the `Request` for one body move; a `#[derive(Clone)]`
struct owning an `RwLock` needs `Arc`-wrapped lock + `impl Clone`; `OffsetDateTime::from_unix_timestamp`
is a `Result`; bind rusqlite string cols to `String` then pass `&s`.

---

## 6. Wiring / file-touch map
- `src/object_format.rs`: optional shared `read_spans_to_stream` (Part B).
- `src/s3_server.rs`: (only if factoring the reader) point S3 `get_object` at the shared helper so it
  stays byte-identical; thread a live transport into `AdminUiState` for the wizard (Part C).
- `src/admin.rs`: route table additions for upload/download + wizard endpoints; keep them all behind
  the existing principal + CSRF gates; add the outbound body seam.
- `src/telegram/login_driver.rs` (new) + `src/telegram/mod.rs` re-exports; touch transport only to
  share internals needed by the driver (keep `interactive_login` working).
- `frontend/src/App.svelte`, `lib/api.ts`, `lib/types.ts` (+ maybe a `components/` split if it grows).
- `Cargo.toml`: only add deps if truly needed for the body/stream adapters; prefer reusing
  `http-body-util`, `bytes`, `futures` already present. Re-check `cargo deny`/`audit`.

## 7. Tests
- Unit: `read_spans_to_stream` equals `read_bytes` result for small objects but must not allocate the
  whole file (assert chunk emission/order); body-adapter round trip posting bytes through an in-process
  req; `login_driver` phase transitions under `TELEGRAM_TRANSPORT_RUNTIME=mock` (begin→authorized, and
  begin→code→2fa→authorized path via a fake `PasswordRequired`).
- Integration (mirror `tests/admin_frontend_smoke.rs` credential boot): seed admin via CLI; upload a
  small object over `/_admin/api/objects/content?bucket&key`; download exact bytes; download `Range:
  bytes=` yields the correct `206` slice; HEAD reports length; guest hit on the content endpoints → 401.
- Wizard integration (mock transport): signed-in admin → wizard begin → submit code → state Authorized
  (or via a forced 2FA stub) → overview reflects authorized; a second concurrent begin → 409.
- Frontend: `npm run check` (0 errors) and `npm run build`.

## 8. Documentation / AGENTS gates
- Update `docs/s3-compatibility.md` Operator UI section (streaming + wizard now implemented).
- Update `docs/adr/0006-multiuser-control-plane.md` consequences + a new `## Update` subsection; flip
  the ADR/ROADMAP "deferred" wording.
- `ROADMAP.md` Phase 9 → note completed next slice.
- `docs/limitations.md`: drop the "not yet wired" lines for transfer/wizard (keep gate wording), add
  explicit future item "bulk folder download / server-side ZIP".
- README status line; `docs/telegram-storage-format.md` stays unchanged (no storage-format change) but
  confirm/record that folder markers are zero-byte objects.
- Update the existing ALL-PHASE9 plan file `.mimocode/plans/1788441564257-eager-forest.md` only if it
  says "next slice" for these items (point it at this file).

## 9. Order of work (recommended) & green bar
1. Land **Part A upload** (smallest, purely additive) + tests; green.
2. Land **Part B download/range** + the shared reader + tests; green (run full S3 suite to prove the
   S3 path is unchanged).
3. Land **Part C wizard** (driver + endpoints + frontend) + tests; green.
4. Docs/ADR/ROADMAP/limitations; final `fmt`/`clippy -D warnings`/`cargo test`/`svelte-check`/`build`.

Definition of done = every §7 test green, clippy/fmt clean, frontend builds, S3 suite unchanged.
