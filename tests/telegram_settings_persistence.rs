use assert_cmd::prelude::*;
use serde_json::Value;
use std::fs;
use std::io::{BufRead, BufReader};
use std::net::TcpListener;
use std::process::{Command, Stdio};
use telegram_s3::manifest::CommittedManifestArgs;
use telegram_s3::object_format::sha256_hex;
use telegram_s3::{CommitState, MetadataStore, ObjectManifest, OperationKind};
use tempfile::TempDir;

fn prepare_admin_ui(tempdir: &TempDir) -> std::path::PathBuf {
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

fn free_bind_addr() -> String {
    let listener = TcpListener::bind("127.0.0.1:0").expect("free port");
    listener.local_addr().expect("local addr").to_string()
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
    command
}

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

fn seed_committed_manifest_without_telegram_settings(tempdir: &TempDir) {
    let store = MetadataStore::open(tempdir.path().join("metadata.sqlite")).expect("metadata");
    let manifest = ObjectManifest::committed(CommittedManifestArgs {
        bucket: "existing".to_string(),
        key: "old.txt".to_string(),
        content_length: 3,
        content_type: "text/plain".to_string(),
        checksum_algorithm: "sha256".to_string(),
        whole_object: sha256_hex(b"old"),
        peer_id: "-1001234567890".to_string(),
        message_id: 1,
    });
    let operation_id = store
        .stage_manifest(OperationKind::Put, manifest)
        .expect("stage manifest");
    let committed = store
        .commit_manifest(operation_id)
        .expect("commit manifest");
    assert_eq!(committed.commit_state, CommitState::Committed);
}

fn wait_listening(reader: &mut BufReader<std::process::ChildStdout>) {
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
            break;
        }
    }
}

fn http_client() -> hyper_util::client::legacy::Client<
    hyper_util::client::legacy::connect::HttpConnector,
    http_body_util::Full<bytes::Bytes>,
> {
    let mut connector = hyper_util::client::legacy::connect::HttpConnector::new();
    connector.enforce_http(false);
    hyper_util::client::legacy::Client::builder(hyper_util::rt::TokioExecutor::new())
        .build(connector)
}

async fn http_request(
    client: &hyper_util::client::legacy::Client<
        hyper_util::client::legacy::connect::HttpConnector,
        http_body_util::Full<bytes::Bytes>,
    >,
    bind_addr: &str,
    method: &str,
    path: &str,
    headers: &[(&str, &str)],
    body: &[u8],
) -> (u16, Vec<(String, String)>, String) {
    let uri = format!("http://{}{}", bind_addr, path);
    let mut request = http::Request::builder()
        .method(method)
        .uri(uri)
        .body(http_body_util::Full::new(bytes::Bytes::copy_from_slice(
            body,
        )))
        .expect("request");
    for (name, value) in headers {
        request.headers_mut().insert(
            http::header::HeaderName::from_bytes(name.as_bytes()).expect("header name"),
            http::header::HeaderValue::from_str(value).expect("header value"),
        );
    }
    let response = client.request(request).await.expect("http");
    let status = response.status().as_u16();
    let headers = response
        .headers()
        .iter()
        .map(|(name, value)| {
            (
                name.as_str().to_string(),
                value.to_str().expect("header value").to_string(),
            )
        })
        .collect::<Vec<_>>();
    let bytes = http_body_util::BodyExt::collect(response.into_body())
        .await
        .expect("body")
        .to_bytes();
    (
        status,
        headers,
        String::from_utf8(bytes.to_vec()).expect("utf8"),
    )
}

fn json_field(body: &str, field: &str) -> String {
    let value: Value = serde_json::from_str(body).expect("json");
    value[field].as_str().expect("field").to_string()
}

fn cookie_value(set_cookie: &str) -> String {
    set_cookie.split(';').next().expect("cookie").to_string()
}

#[tokio::test]
async fn telegram_settings_survive_restart_without_bootstrap_envs() {
    let tempdir = TempDir::new().expect("tempdir");
    seed_admin_users(&tempdir);

    let bind_addr = free_bind_addr();
    let mut server_command = command_for(&tempdir, &bind_addr);
    server_command.arg("server");
    server_command.stdout(Stdio::piped());
    let mut child = server_command.spawn().expect("spawn server");
    let stdout = child.stdout.take().expect("server stdout");
    let mut reader = BufReader::new(stdout);
    wait_listening(&mut reader);

    let client = http_client();
    let login_body = br#"{"username":"admin","password":"correct-horse-battery-staple"}"#;
    let (status, headers, body) = http_request(
        &client,
        &bind_addr,
        "POST",
        "/_admin/api/session/login",
        &[],
        login_body,
    )
    .await;
    assert_eq!(status, 200);
    let csrf = json_field(&body, "csrf_token");
    let cookie_header = headers
        .iter()
        .find(|(name, _)| name.eq_ignore_ascii_case("set-cookie"))
        .map(|(_, value)| cookie_value(value))
        .expect("session cookie");

    let payload = serde_json::json!({
        "telegram_api_id": "12345",
        "telegram_api_hash": "hash",
        "telegram_session_path": tempdir.path().join("telegram.session").display().to_string(),
        "telegram_storage_chat_id": "-1001234567890",
        "telegram_proxy_url": "socks5://127.0.0.1:12334",
        "telegram_proxy_username": "",
        "telegram_proxy_password": "",
        "telegram_proxy_mode": "auto"
    });
    let (status, _headers, saved_body) = http_request(
        &client,
        &bind_addr,
        "POST",
        "/_admin/api/telegram/settings",
        &[
            ("Cookie", cookie_header.as_str()),
            ("X-CSRF-Token", csrf.as_str()),
        ],
        payload.to_string().as_bytes(),
    )
    .await;
    assert_eq!(status, 200);
    let saved: Value = serde_json::from_str(&saved_body).expect("saved json");
    assert_eq!(saved["settings"]["telegram_api_id"], "12345");
    assert_eq!(
        saved["settings"]["telegram_storage_chat_id"],
        "-1001234567890"
    );

    let _ = child.kill();
    let _ = child.wait();

    let mut restart = command_for(&tempdir, &bind_addr);
    restart.arg("server");
    restart.stdout(Stdio::piped());
    let mut child = restart.spawn().expect("restart server");
    let stdout = child.stdout.take().expect("server stdout");
    let mut reader = BufReader::new(stdout);
    wait_listening(&mut reader);

    let client = http_client();
    let (status, headers, body) = http_request(
        &client,
        &bind_addr,
        "POST",
        "/_admin/api/session/login",
        &[],
        login_body,
    )
    .await;
    assert_eq!(status, 200);
    let csrf = json_field(&body, "csrf_token");
    let cookie_header = headers
        .iter()
        .find(|(name, _)| name.eq_ignore_ascii_case("set-cookie"))
        .map(|(_, value)| cookie_value(value))
        .expect("session cookie");

    let (status, _headers, settings_body) = http_request(
        &client,
        &bind_addr,
        "GET",
        "/_admin/api/telegram/settings",
        &[
            ("Cookie", cookie_header.as_str()),
            ("X-CSRF-Token", csrf.as_str()),
        ],
        b"",
    )
    .await;
    assert_eq!(status, 200);
    let settings: Value = serde_json::from_str(&settings_body).expect("settings json");
    assert_eq!(settings["settings"]["telegram_api_id"], "12345");
    assert_eq!(
        settings["settings"]["telegram_storage_chat_id"],
        "-1001234567890"
    );

    let _ = child.kill();
    let _ = child.wait();
}

#[tokio::test]
async fn server_boots_existing_metadata_without_telegram_settings() {
    let tempdir = TempDir::new().expect("tempdir");
    seed_admin_users(&tempdir);
    seed_committed_manifest_without_telegram_settings(&tempdir);

    let bind_addr = free_bind_addr();
    let mut server_command = command_for(&tempdir, &bind_addr);
    server_command.arg("server");
    server_command.stdout(Stdio::piped());
    let mut child = server_command.spawn().expect("spawn server");
    let stdout = child.stdout.take().expect("server stdout");
    let mut reader = BufReader::new(stdout);
    wait_listening(&mut reader);

    let exited = child.try_wait().expect("try wait");
    assert!(exited.is_none(), "server exited before admin setup");

    let _ = child.kill();
    let _ = child.wait();
}
