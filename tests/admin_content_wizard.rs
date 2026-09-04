//! Integration coverage for the /_admin content ("objects/content") endpoints and
//! the Telegram login wizard (_admin/api/telegram/wizard/*).
//!
//! One server process is booted against the mock transport with
//! `TELEGRAM_MOCK_FORCE_2FA=1` so the wizard's cloud-password branch can be driven
//! deterministically. An admin operator is seeded into the same metadata store the
//! server will use, then a real operator session is established over HTTP. A unique
//! bucket is first created over the S3 data plane (SigV4 signed PUT on the shared
//! public listener), then raw byte content is uploaded and read back through the
//! admin content route. After that the staged login wizard is exercised through all
//! phases (idle -> code -> two_fa -> authorized) and cancelled.
//!
//! Content reads are asserted on raw `Vec<u8>` bytes (byte-preserving, not lossy
//! UTF-8) so the full-body / range equality checks prove fidelity. The wizard and
//! content routes share the same logged-in cookie + CSRF token.

use assert_cmd::prelude::*;
use aws_credential_types::Credentials;
use aws_sigv4::http_request::{
    PayloadChecksumKind, PercentEncodingMode, SignableBody, SignableRequest, SigningSettings,
    UriPathNormalizationMode, sign,
};
use aws_sigv4::sign::v4;
use bytes::Bytes;
use http::Request;
use http_body_util::{BodyExt, Full};
use hyper_util::client::legacy::Client;
use hyper_util::client::legacy::connect::HttpConnector;
use hyper_util::rt::TokioExecutor;
use serde_json::Value;
use std::collections::HashMap;
use std::fs;
use std::io::{BufRead, BufReader};
use std::net::TcpListener;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::time::SystemTime;
use telegram_s3::metadata::{MetadataStore, TelegramBootstrapSettings};
use tempfile::TempDir;

/// Public endpoints under /_admin share the FIRST `listening on <addr>` line with
/// the S3 data plane. This helper frees an OS port to hand the server for binding.
fn free_bind_addr() -> String {
    let listener = TcpListener::bind("127.0.0.1:0").expect("free port");
    listener.local_addr().expect("local addr").to_string()
}

fn prepare_admin_ui(tempdir: &TempDir) -> PathBuf {
    let ui_dir = tempdir.path().join("ui");
    let assets_dir = ui_dir.join("assets");
    fs::create_dir_all(&assets_dir).expect("ui dir");
    fs::write(
        ui_dir.join("index.html"),
        "<!doctype html><html><body>telegram-s3 admin</body></html>",
    )
    .expect("ui index");
    fs::write(assets_dir.join("app.css"), "body{}").expect("ui asset");
    ui_dir
}

/// Seed an operator account into the metadata store BEFORE the server boots, the
/// way production pairs the CLI seed with the running admin.
fn seed_admin_users(tempdir: &TempDir) {
    let mut cli = Command::cargo_bin("telegram-s3").expect("binary");
    cli.env(
        "TELEGRAM_METADATA_PATH",
        tempdir.path().join("metadata.sqlite"),
    );
    cli.arg("users");
    cli.arg("create");
    cli.arg("admin");
    cli.args(["--password", "correct-horse-battery-staple"]);
    let output = cli.output().expect("seed admin");
    assert!(
        output.status.success(),
        "seed failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

fn seed_telegram_settings(tempdir: &TempDir) {
    let store = MetadataStore::open(tempdir.path().join("metadata.sqlite")).expect("metadata");
    store
        .set_telegram_bootstrap_settings(&TelegramBootstrapSettings {
            telegram_api_id: Some("12345".to_string()),
            telegram_api_hash: Some("hash".to_string()),
            telegram_storage_chat_id: Some("-1001234567890".to_string()),
            telegram_proxy_mode: Some("auto".to_string()),
            ..TelegramBootstrapSettings::default()
        })
        .expect("telegram settings");
}

fn command_for(tempdir: &TempDir, bind_addr: &str) -> Command {
    let metadata_path = tempdir.path().join("metadata.sqlite");
    let data_dir = tempdir.path().join("data");
    let ui_dir = prepare_admin_ui(tempdir);
    fs::create_dir_all(&data_dir).expect("data dir");

    let mut command = Command::cargo_bin("telegram-s3").expect("binary");
    command.env(
        "TELEGRAM_METADATA_PATH",
        metadata_path.display().to_string(),
    );
    command.env("TELEGRAM_DATA_DIR", data_dir.display().to_string());
    command.env("TELEGRAM_S3_BIND_ADDR", bind_addr);
    command.env("TELEGRAM_S3_MASTER_KEY", "master-key");
    command.env("RUSTFS_ACCESS_KEY", "access-key");
    command.env("RUSTFS_SECRET_KEY", "secret-key");
    command.env("TELEGRAM_ADMIN_BOOTSTRAP_SECRET", "bootstrap-secret");
    command.env("TELEGRAM_ADMIN_UI_DIST_DIR", ui_dir.display().to_string());
    command.env("TELEGRAM_FLOOD_WAIT_RESPECT", "true");
    command.env("TELEGRAM_CHUNK_SIZE", "1048576");
    command.env("TELEGRAM_CONNECTION_TIMEOUT_SECS", "30");
    command.env("TELEGRAM_REQUEST_TIMEOUT_SECS", "30");
    command.env("TELEGRAM_TRANSFER_TIMEOUT_SECS", "900");
    command.env("TELEGRAM_RETRY_COUNT", "5");
    command.env("TELEGRAM_RETRY_BACKOFF_MS", "500");
    command.env("TELEGRAM_ADMIN_BIND_ADDR", "127.0.0.1:0");
    command.env("TELEGRAM_TRANSPORT_RUNTIME", "mock");
    // Force the mock Telegram account to demand a cloud password so the wizard
    // 2FA branch is exercised end to end.
    command.env("TELEGRAM_MOCK_FORCE_2FA", "1");
    command
}

#[tokio::test]
async fn admin_content_roundtrip_and_login_wizard_phases() {
    let tempdir = TempDir::new().expect("tempdir");
    seed_admin_users(&tempdir);
    seed_telegram_settings(&tempdir);

    let bind_addr = free_bind_addr();
    let mut server_command = command_for(&tempdir, &bind_addr);
    server_command.arg("server");
    server_command.stdout(Stdio::piped());
    let mut child = server_command.spawn().expect("spawn server");
    let stdout = child.stdout.take().expect("server stdout");
    let mut reader = BufReader::new(stdout);
    let mut saw_listening = false;
    let mut line = String::new();
    for _ in 0..50 {
        line.clear();
        let bytes = reader.read_line(&mut line).expect("server line");
        if bytes == 0 {
            break;
        }
        if line
            .trim_end_matches(['\r', '\n'])
            .starts_with("listening on ")
        {
            saw_listening = true;
            break;
        }
    }
    assert!(
        saw_listening,
        "server never printed a public listening addr"
    );

    let client = http_client();

    // ---- operator login ------------------------------------------------------
    let login = raw_request(
        &client,
        &bind_addr,
        "POST",
        "/_admin/api/session/login",
        &[],
        br#"{"username":"admin","password":"correct-horse-battery-staple"}"#,
    )
    .await;
    assert_eq!(login.status, 200, "login status");
    let login_json: Value = body_json(&login.body);
    assert_eq!(login_json["authenticated"], Value::Bool(true), "login body");
    let csrf = login_json["csrf_token"]
        .as_str()
        .expect("csrf token string")
        .to_string();
    let cookie = cookie_value(
        &login
            .headers
            .get("set-cookie")
            .cloned()
            .expect("session set-cookie"),
    );

    // ---- content route, backed by a freshly created S3 bucket ----------------
    let bucket = format!("wiz-{}", uuid::Uuid::new_v4().simple());
    let key = "folder/hello.txt";

    // Create the bucket through the S3 data plane (same shared listener).
    let create = s3_signed_request(&bind_addr, "PUT", &format!("/{bucket}"), None, &[], &[]).await;
    assert_eq!(create.status, 200, "S3 create bucket");

    // Raw, deliberately non-UTF-8 content so equality proves byte preservation.
    let raw = (200u8..=254u8).collect::<Vec<u8>>();
    assert!(raw.len() >= 16);

    let q = encode_query(&[("bucket", &bucket), ("key", key)]);
    let upload_path = format!("/_admin/api/objects/content?{q}");

    // Upload raw bytes with Content-Type application/octet-stream.
    let upload = raw_request(
        &client,
        &bind_addr,
        "POST",
        &upload_path,
        &[
            ("Cookie", &cookie),
            ("X-CSRF-Token", &csrf),
            ("Content-Type", "application/octet-stream"),
        ],
        &raw,
    )
    .await;
    assert_eq!(upload.status, 200, "content upload");
    let upload_json = body_json(&upload.body);
    assert_eq!(
        upload_json["size"].as_u64(),
        Some(raw.len() as u64),
        "upload size"
    );
    assert!(
        upload_json["etag"].is_string() && !upload_json["etag"].as_str().unwrap().is_empty(),
        "upload etag"
    );
    assert!(upload_json["version_id"].is_string(), "upload version_id");

    // Full GET returns the exact bytes and the expected headers.
    let full = raw_request(
        &client,
        &bind_addr,
        "GET",
        &upload_path,
        &[("Cookie", &cookie)],
        b"",
    )
    .await;
    assert_eq!(full.status, 200, "content full GET");
    assert_eq!(full.body, raw, "full GET body must equal raw bytes");
    assert_eq!(header(&full, "content-type"), "application/octet-stream");
    assert_eq!(
        header(&full, "content-length"),
        raw.len().to_string(),
        "full content-length"
    );
    assert!(header(&full, "etag") == upload_json["etag"].as_str().unwrap());
    assert!(
        header(&full, "content-disposition").contains("attachment"),
        "content-disposition should be an attachment, got {:?}",
        full.headers.get("content-disposition")
    );

    // Byte range GET: bytes=4-8 -> start offset 4 through end offset 8 inclusive
    // (5 bytes served). Content-Range advertises "bytes 4-8/<len>".
    let ranged = raw_request(
        &client,
        &bind_addr,
        "GET",
        &upload_path,
        &[("Cookie", &cookie), ("Range", "bytes=4-8")],
        b"",
    )
    .await;
    assert_eq!(ranged.status, 206, "ranged GET status");
    assert_eq!(
        ranged.body,
        raw[4..=8],
        "ranged GET body must be raw[4..=8]"
    );
    assert_eq!(
        header(&ranged, "content-range"),
        format!("bytes 4-8/{}", raw.len())
    );

    // HEAD reflects the stored length, empty body.
    let head = raw_request(
        &client,
        &bind_addr,
        "HEAD",
        &upload_path,
        &[("Cookie", &cookie)],
        b"",
    )
    .await;
    assert_eq!(head.status, 200, "content HEAD status");
    assert_eq!(
        header(&head, "content-length"),
        raw.len().to_string(),
        "HEAD content-length"
    );
    assert!(head.body.is_empty(), "HEAD body should be empty");

    // GET on a missing object -> 404.
    let missing_q = encode_query(&[("bucket", &bucket), ("key", "folder/absent.txt")]);
    let missing = raw_request(
        &client,
        &bind_addr,
        "GET",
        &format!("/_admin/api/objects/content?{missing_q}"),
        &[("Cookie", &cookie)],
        b"",
    )
    .await;
    assert_eq!(missing.status, 404, "content GET missing must be 404");

    // An unauthenticated (guest) reader is rejected.
    let guest = raw_request(&client, &bind_addr, "GET", &upload_path, &[], b"").await;
    assert_eq!(guest.status, 401, "guest content GET must be 401");

    // ---- Telegram login wizard phases ----------------------------------------
    // Start: idle, with an authorized flag on the wire.
    let state = json_request(
        &client,
        &bind_addr,
        "GET",
        "/_admin/api/telegram/wizard/state",
        &[("Cookie", &cookie)],
        b"",
    )
    .await
    .bytes;
    let state_json = body_json(&state);
    assert_eq!(state_json["phase"], Value::String("idle".to_string()));
    assert!(
        state_json.get("authorized").is_some(),
        "state has authorized"
    );

    // begin -> code.
    let began = json_request(
        &client,
        &bind_addr,
        "POST",
        "/_admin/api/telegram/wizard/begin",
        &[
            ("Cookie", &cookie),
            ("X-CSRF-Token", &csrf),
            ("Content-Type", "application/json"),
        ],
        br#"{"phone":"+15551234567"}"#,
    )
    .await;
    assert_eq!(began.status_code, 200, "wizard begin status");
    let began_json = began.bytes_json;
    assert_eq!(began_json["phase"], Value::String("code".to_string()));
    assert_eq!(began_json["needs_2fa"], Value::Bool(false));

    // A second begin while a flow is mid-stage is rejected (occupied, 409).
    let rebegin = json_request(
        &client,
        &bind_addr,
        "POST",
        "/_admin/api/telegram/wizard/begin",
        &[
            ("Cookie", &cookie),
            ("X-CSRF-Token", &csrf),
            ("Content-Type", "application/json"),
        ],
        br#"{"phone":"+15559998877"}"#,
    )
    .await;
    assert_eq!(rebegin.status_code, 409, "wizard re-begin status");

    // submit-code (forced 2FA) -> two_fa, needs_2fa true.
    let two_fa = json_request(
        &client,
        &bind_addr,
        "POST",
        "/_admin/api/telegram/wizard/submit-code",
        &[
            ("Cookie", &cookie),
            ("X-CSRF-Token", &csrf),
            ("Content-Type", "application/json"),
        ],
        br#"{"code":"12345"}"#,
    )
    .await;
    assert_eq!(two_fa.status_code, 200, "submit-code status");
    assert_eq!(
        two_fa.bytes_json["phase"],
        Value::String("two_fa".to_string())
    );
    assert_eq!(two_fa.bytes_json["needs_2fa"], Value::Bool(true));

    // submit-password -> authorized.
    let authorized = json_request(
        &client,
        &bind_addr,
        "POST",
        "/_admin/api/telegram/wizard/submit-password",
        &[
            ("Cookie", &cookie),
            ("X-CSRF-Token", &csrf),
            ("Content-Type", "application/json"),
        ],
        br#"{"password":"anything"}"#,
    )
    .await;
    assert_eq!(authorized.status_code, 200, "submit-password status");
    assert_eq!(
        authorized.bytes_json["phase"],
        Value::String("authorized".to_string())
    );
    assert_eq!(authorized.bytes_json["authorized"], Value::Bool(true));

    // State reflects authorization.
    let state_after = json_request(
        &client,
        &bind_addr,
        "GET",
        "/_admin/api/telegram/wizard/state",
        &[("Cookie", &cookie)],
        b"",
    )
    .await
    .bytes;
    let state_after = body_json(&state_after);
    assert_eq!(
        state_after["phase"],
        Value::String("authorized".to_string())
    );
    assert_eq!(state_after["authorized"], Value::Bool(true));

    // Cancel the wizard.
    let cancel = json_request(
        &client,
        &bind_addr,
        "POST",
        "/_admin/api/telegram/wizard/cancel",
        &[
            ("Cookie", &cookie),
            ("X-CSRF-Token", &csrf),
            ("Content-Type", "application/json"),
        ],
        b"",
    )
    .await;
    assert_eq!(cancel.status_code, 200, "cancel status");
    assert_eq!(cancel.bytes_json["ok"], Value::Bool(true));

    let _ = child.kill();
    let _ = child.wait();
}

// ---- HTTP plumbing -----------------------------------------------------------

struct RawResponse {
    status: u16,
    headers: HashMap<String, String>,
    body: Vec<u8>,
}

/// JSON-facing response keeps the parsed JSON plus raw bytes so callers that need
/// to also grab headers/cookies can fall back to the raw helper.
struct JsonResponse {
    status_code: u16,
    bytes: Vec<u8>,
    bytes_json: Value,
}

fn http_client() -> Client<HttpConnector, Full<Bytes>> {
    let connector = HttpConnector::new();
    Client::builder(TokioExecutor::new()).build(connector)
}

fn cookie_value(header: &str) -> String {
    header
        .split(';')
        .next()
        .unwrap_or(header)
        .trim()
        .to_string()
}

fn header(resp: &RawResponse, name: &str) -> String {
    resp.headers
        .get(name)
        .cloned()
        .unwrap_or_default()
        .to_string()
}

fn body_json(bytes: &[u8]) -> Value {
    serde_json::from_slice(bytes).unwrap_or(Value::Null)
}

/// Percent-encode query pairs the way form_urlencoded produces them, matching the
/// server's `parse_list_params` decoding on the far side.
fn encode_query(pairs: &[(&str, &str)]) -> String {
    let mut serializer = url::form_urlencoded::Serializer::new(String::new());
    for (name, value) in pairs {
        serializer.append_pair(name, value);
    }
    serializer.finish()
}

/// Byte-preserving request helper: takes and returns raw bytes (never lossy UTF-8).
async fn raw_request(
    client: &Client<HttpConnector, Full<Bytes>>,
    addr: &str,
    method: &str,
    path_and_query: &str,
    headers: &[(&str, &str)],
    body: &[u8],
) -> RawResponse {
    let mut request = Request::builder()
        .method(method)
        .uri(format!("http://{addr}{path_and_query}"))
        .header("Host", addr)
        .header("Connection", "close");
    for (name, value) in headers {
        if value.is_empty() {
            continue;
        }
        request = request.header(*name, *value);
    }
    request = request.header("Content-Length", body.len().to_string());
    let response = client
        .request(
            request
                .body(Full::new(Bytes::copy_from_slice(body)))
                .expect("request body"),
        )
        .await
        .expect("request executed");
    let status = response.status().as_u16();
    let mut headers = HashMap::new();
    for (name, value) in response.headers() {
        headers.insert(
            name.as_str().to_ascii_lowercase(),
            value.to_str().unwrap_or_default().to_string(),
        );
    }
    let body = response
        .into_body()
        .collect()
        .await
        .expect("read response body")
        .to_bytes()
        .to_vec();
    RawResponse {
        status,
        headers,
        body,
    }
}

/// JSON convenience over [`raw_request`].
async fn json_request(
    client: &Client<HttpConnector, Full<Bytes>>,
    addr: &str,
    method: &str,
    path: &str,
    headers: &[(&str, &str)],
    body: &[u8],
) -> JsonResponse {
    let raw = raw_request(client, addr, method, path, headers, body).await;
    let code = raw.status;
    let json = body_json(&raw.body);
    JsonResponse {
        status_code: code,
        bytes: raw.body,
        bytes_json: json,
    }
}

/// AWS SigV4-signed S3 request for creating the bucket over the shared listener.
async fn s3_signed_request(
    addr: &str,
    method: &str,
    path: &str,
    query: Option<&str>,
    extra_headers: &[(&str, &str)],
    body: &[u8],
) -> RawResponse {
    let request_target = match query {
        Some(query) => format!("{path}?{query}"),
        None => path.to_string(),
    };
    let request_uri = format!("http://{addr}{request_target}");
    let host_header = addr.to_string();
    let mut request = Request::builder()
        .method(method)
        .uri(&request_uri)
        .header("host", &host_header);
    for (name, value) in extra_headers {
        request = request.header(*name, *value);
    }
    let request = request
        .body(Full::new(Bytes::copy_from_slice(body)))
        .expect("build request");

    let identity = Credentials::new(
        "access-key",
        "secret-key",
        None,
        None,
        "telegram-s3-admin-content-wizard",
    )
    .into();
    let mut signing_settings = SigningSettings::default();
    signing_settings.payload_checksum_kind = PayloadChecksumKind::XAmzSha256;
    signing_settings.percent_encoding_mode = PercentEncodingMode::Single;
    signing_settings.uri_path_normalization_mode = UriPathNormalizationMode::Disabled;
    let signing_params = v4::SigningParams::builder()
        .identity(&identity)
        .region("us-east-1")
        .name("s3")
        .time(SystemTime::now())
        .settings(signing_settings)
        .build()
        .expect("signing params")
        .into();
    let signable_request = SignableRequest::new(
        method,
        &request_uri,
        request
            .headers()
            .iter()
            .map(|(name, value)| (name.as_str(), value.to_str().expect("header utf8"))),
        SignableBody::Bytes(body),
    )
    .expect("signable request");
    let (signing_instructions, _signature) = sign(signable_request, &signing_params)
        .expect("sign request")
        .into_parts();
    let mut request = request;
    signing_instructions.apply_to_request_http1x(&mut request);

    let client: Client<HttpConnector, Full<Bytes>> =
        Client::builder(TokioExecutor::new()).build_http();
    let response = client.request(request).await.expect("send request");
    let status = response.status().as_u16();
    let mut headers = HashMap::new();
    for (name, value) in response.headers() {
        headers.insert(
            name.as_str().to_ascii_lowercase(),
            value.to_str().expect("header utf8").to_string(),
        );
    }
    let body = response
        .into_body()
        .collect()
        .await
        .expect("read response body")
        .to_bytes()
        .to_vec();
    RawResponse {
        status,
        headers,
        body,
    }
}
