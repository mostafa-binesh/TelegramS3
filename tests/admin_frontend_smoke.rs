use assert_cmd::prelude::*;
use bytes::Bytes;
use http::Request;
use http_body_util::{BodyExt, Full};
use hyper_util::client::legacy::{Client, connect::HttpConnector};
use hyper_util::rt::TokioExecutor;
use serde_json::Value;
use std::fs;
use std::io::{BufRead, BufReader};
use std::net::TcpListener;
use std::process::{Command, Stdio};
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
    let session_path = tempdir.path().join("telegram.session");
    let data_dir = tempdir.path().join("data");
    let ui_dir = prepare_admin_ui(tempdir);
    fs::create_dir_all(&data_dir).expect("data dir");

    let mut command = Command::cargo_bin("telegram-s3").expect("binary");
    command.env("TELEGRAM_API_ID", "12345");
    command.env("TELEGRAM_API_HASH", "hash");
    command.env("TELEGRAM_PHONE_NUMBER", "+15551234567");
    command.env("TELEGRAM_SESSION_PATH", session_path.display().to_string());
    command.env("TELEGRAM_STORAGE_CHAT_ID", "-1001234567890");
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
    command.env("TELEGRAM_PROXY_MODE", "auto");
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

#[tokio::test]
async fn authenticated_admin_surface_serves_dashboard_and_session_lifecycle() {
    let tempdir = TempDir::new().expect("tempdir");
    let bind_addr = free_bind_addr();

    let mut server_command = command_for(&tempdir, &bind_addr);
    server_command.arg("server");
    server_command.stdout(Stdio::piped());
    let mut child = server_command.spawn().expect("spawn server");
    let stdout = child.stdout.take().expect("server stdout");
    let mut reader = BufReader::new(stdout);
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

    let client = http_client();

    let session = http_request(&client, &bind_addr, "GET", "/_admin/api/session", &[], b"").await;
    assert_eq!(session.status, 200);
    assert!(session.body.contains("\"authenticated\":false"));

    let login = http_request(
        &client,
        &bind_addr,
        "POST",
        "/_admin/api/session/login",
        &[],
        br#"{"bootstrap_secret":"bootstrap-secret"}"#,
    )
    .await;
    assert_eq!(login.status, 200);
    assert!(login.body.contains("\"authenticated\":true"));
    let cookie = login
        .headers
        .iter()
        .find(|(name, _)| name.eq_ignore_ascii_case("set-cookie"))
        .map(|(_, value)| value.clone())
        .expect("session cookie");
    let csrf = json_field(&login.body, "csrf_token").expect("csrf token");
    let cookie_header = cookie_value(&cookie);

    let overview = http_request(
        &client,
        &bind_addr,
        "GET",
        "/_admin/api/overview",
        &[("Cookie", cookie_header.as_str())],
        b"",
    )
    .await;
    assert_eq!(overview.status, 200);
    assert!(overview.body.contains("\"endpoint\""));
    assert!(overview.body.contains("\"bootstrap\""));

    let refresh = http_request(
        &client,
        &bind_addr,
        "POST",
        "/_admin/api/session/refresh",
        &[
            ("Cookie", cookie_header.as_str()),
            ("X-CSRF-Token", csrf.as_str()),
        ],
        b"",
    )
    .await;
    assert_eq!(refresh.status, 200);
    assert!(refresh.body.contains("\"authenticated\":true"));

    let logout = http_request(
        &client,
        &bind_addr,
        "POST",
        "/_admin/api/session/logout",
        &[
            ("Cookie", cookie_header.as_str()),
            ("X-CSRF-Token", csrf.as_str()),
        ],
        b"",
    )
    .await;
    assert_eq!(logout.status, 200);
    assert!(logout.body.contains("\"authenticated\":false"));

    let spa = http_request(&client, &bind_addr, "GET", "/_admin/", &[], b"").await;
    assert_eq!(spa.status, 200);
    assert!(spa.body.contains("telegram-s3 admin"));

    let _ = child.kill();
    let _ = child.wait();
}

struct HttpResponse {
    status: u16,
    headers: Vec<(String, String)>,
    body: String,
}

fn http_client() -> Client<HttpConnector, Full<Bytes>> {
    let connector = HttpConnector::new();
    Client::builder(TokioExecutor::new()).build(connector)
}

async fn http_request(
    client: &Client<HttpConnector, Full<Bytes>>,
    addr: &str,
    method: &str,
    path: &str,
    headers: &[(&str, &str)],
    body: &[u8],
) -> HttpResponse {
    let mut request = Request::builder()
        .method(method)
        .uri(format!("http://{addr}{path}"))
        .header("Host", addr)
        .header("Connection", "close");
    for (name, value) in headers {
        request = request.header(*name, *value);
    }
    let response = client
        .request(
            request
                .body(Full::from(Bytes::copy_from_slice(body)))
                .expect("request body"),
        )
        .await
        .expect("request");
    let status = response.status().as_u16();
    let headers = response
        .headers()
        .iter()
        .map(|(name, value)| {
            (
                name.as_str().to_string(),
                value.to_str().unwrap_or_default().to_string(),
            )
        })
        .collect();
    let body = response
        .into_body()
        .collect()
        .await
        .expect("body")
        .to_bytes();
    HttpResponse {
        status,
        headers,
        body: String::from_utf8(body.to_vec()).expect("utf8"),
    }
}

fn json_field(body: &str, field: &str) -> Option<String> {
    let value: Value = serde_json::from_str(body).ok()?;
    value.get(field)?.as_str().map(ToString::to_string)
}

fn cookie_value(header: &str) -> String {
    header
        .split(';')
        .next()
        .unwrap_or(header)
        .trim()
        .to_string()
}
