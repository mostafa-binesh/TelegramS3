use assert_cmd::prelude::*;
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
use std::process::{Command, Stdio};
use std::time::Duration;
use telegram_s3::metadata::{MetadataStore, TelegramBootstrapSettings};
use tempfile::TempDir;

struct ResponseData {
    status: u16,
    headers: HashMap<String, String>,
    body: Vec<u8>,
}

fn free_addr() -> String {
    let listener = TcpListener::bind("127.0.0.1:0").expect("free port");
    listener.local_addr().expect("local addr").to_string()
}

fn seed(temp: &TempDir) {
    let mut cli = Command::cargo_bin("telegram-s3").expect("binary");
    cli.env(
        "TELEGRAM_METADATA_PATH",
        temp.path().join("metadata.sqlite"),
    );
    cli.args([
        "users",
        "create",
        "admin",
        "--password",
        "correct-horse-battery-staple",
    ]);
    let output = cli.output().expect("seed admin");
    assert!(
        output.status.success(),
        "seed failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let store = MetadataStore::open(temp.path().join("metadata.sqlite")).expect("metadata");
    store
        .set_telegram_bootstrap_settings(&TelegramBootstrapSettings {
            telegram_api_id: Some("12345".into()),
            telegram_api_hash: Some("hash".into()),
            telegram_storage_chat_id: Some("-1001234567890".into()),
            telegram_proxy_mode: Some("auto".into()),
            ..Default::default()
        })
        .expect("telegram settings");
}

fn command_for(temp: &TempDir, addr: &str) -> Command {
    let ui = temp.path().join("ui");
    fs::create_dir_all(ui.join("assets")).expect("ui dirs");
    fs::write(ui.join("index.html"), "<html>admin</html>").expect("index");
    fs::write(ui.join("assets/app.css"), "body{}").expect("asset");
    let mut command = Command::cargo_bin("telegram-s3").expect("binary");
    command.env(
        "TELEGRAM_METADATA_PATH",
        temp.path().join("metadata.sqlite"),
    );
    command.env("TELEGRAM_DATA_DIR", temp.path().join("data"));
    command.env("TELEGRAM_S3_BIND_ADDR", addr);
    command.env("TELEGRAM_S3_MASTER_KEY", "master-key");
    command.env("RUSTFS_ACCESS_KEY", "access-key");
    command.env("RUSTFS_SECRET_KEY", "secret-key");
    command.env("TELEGRAM_ADMIN_BOOTSTRAP_SECRET", "bootstrap-secret");
    command.env("TELEGRAM_ADMIN_UI_DIST_DIR", ui);
    command.env("TELEGRAM_ADMIN_BIND_ADDR", "127.0.0.1:0");
    command.env("TELEGRAM_TRANSPORT_RUNTIME", "mock");
    command.env("TELEGRAM_CHUNK_SIZE", "8");
    command.env("TELEGRAM_CONNECTION_TIMEOUT_SECS", "30");
    command.env("TELEGRAM_REQUEST_TIMEOUT_SECS", "30");
    command.env("TELEGRAM_TRANSFER_TIMEOUT_SECS", "60");
    command
}

fn client() -> Client<HttpConnector, Full<Bytes>> {
    Client::builder(TokioExecutor::new()).build_http()
}

fn json(body: &[u8]) -> Value {
    serde_json::from_slice(body).expect("json response")
}

fn cookie(response: &ResponseData) -> String {
    response
        .headers
        .get("set-cookie")
        .expect("set-cookie")
        .split(';')
        .next()
        .unwrap()
        .to_string()
}

async fn request(
    client: &Client<HttpConnector, Full<Bytes>>,
    addr: &str,
    method: &str,
    path: &str,
    headers: &[(&str, &str)],
    body: &[u8],
) -> ResponseData {
    let mut builder = Request::builder()
        .method(method)
        .uri(format!("http://{addr}{path}"))
        .header("Host", addr)
        .header("Connection", "close");
    for (name, value) in headers {
        builder = builder.header(*name, *value);
    }
    let response = client
        .request(
            builder
                .body(Full::new(Bytes::copy_from_slice(body)))
                .expect("request"),
        )
        .await
        .expect("response");
    let status = response.status().as_u16();
    let headers = response
        .headers()
        .iter()
        .map(|(name, value)| {
            (
                name.as_str().to_ascii_lowercase(),
                value.to_str().unwrap_or_default().to_string(),
            )
        })
        .collect();
    let body = response
        .into_body()
        .collect()
        .await
        .expect("body")
        .to_bytes()
        .to_vec();
    ResponseData {
        status,
        headers,
        body,
    }
}

#[tokio::test]
async fn resumable_reception_roundtrip_offset_recovery_and_abort() {
    let temp = TempDir::new().expect("tempdir");
    seed(&temp);
    let addr = free_addr();
    let mut command = command_for(&temp, &addr);
    command.arg("server").stdout(Stdio::piped());
    let mut child = command.spawn().expect("server");
    let stdout = child.stdout.take().expect("stdout");
    let mut reader = BufReader::new(stdout);
    let mut line = String::new();
    assert!(
        (0..50).any(|_| {
            line.clear();
            reader.read_line(&mut line).unwrap_or(0) > 0 && line.starts_with("listening on ")
        }),
        "server did not start"
    );
    let http = client();

    let login = request(
        &http,
        &addr,
        "POST",
        "/_admin/api/session/login",
        &[("Content-Type", "application/json")],
        br#"{"username":"admin","password":"correct-horse-battery-staple"}"#,
    )
    .await;
    assert_eq!(login.status, 200);
    let login_json = json(&login.body);
    let csrf = login_json["csrf_token"].as_str().unwrap().to_string();
    let session_cookie = cookie(&login);
    let auth = [
        ("Cookie", session_cookie.as_str()),
        ("X-CSRF-Token", csrf.as_str()),
        ("Content-Type", "application/json"),
    ];
    let bucket = format!("resume-{}", uuid::Uuid::new_v4().simple());
    let create = request(
        &http,
        &addr,
        "POST",
        "/_admin/api/buckets",
        &auth,
        format!(r#"{{"name":"{bucket}"}}"#).as_bytes(),
    )
    .await;
    assert_eq!(create.status, 201);

    let begin = request(&http, &addr, "POST", "/_admin/api/uploads/resumable", &auth, format!(r#"{{"bucket":"{bucket}","key":"folder/file.bin","content_type":"application/octet-stream"}}"#).as_bytes()).await;
    assert_eq!(begin.status, 201);
    let begin_json = json(&begin.body);
    let id = begin_json["id"].as_str().unwrap().to_string();
    assert_eq!(begin_json["chunk_size"], 8);
    let patch_headers = [
        ("Cookie", session_cookie.as_str()),
        ("X-CSRF-Token", csrf.as_str()),
        ("Content-Type", "application/octet-stream"),
    ];
    let first = request(
        &http,
        &addr,
        "PATCH",
        &format!("/_admin/api/uploads/resumable/{id}?offset=0&final=0"),
        &patch_headers,
        &[1, 2, 3, 4, 5, 6, 7, 8],
    )
    .await;
    assert_eq!(first.status, 200);
    assert_eq!(json(&first.body)["received"], 8);
    let stale = request(
        &http,
        &addr,
        "PATCH",
        &format!("/_admin/api/uploads/resumable/{id}?offset=0&final=0"),
        &patch_headers,
        &[1, 2, 3, 4, 5, 6, 7, 8],
    )
    .await;
    assert_eq!(stale.status, 409);
    assert_eq!(json(&stale.body)["received"], 8);
    let second = request(
        &http,
        &addr,
        "PATCH",
        &format!("/_admin/api/uploads/resumable/{id}?offset=8&final=0"),
        &patch_headers,
        &[9, 10, 11, 12, 13, 14, 15, 16],
    )
    .await;
    assert_eq!(second.status, 200);
    let last = request(
        &http,
        &addr,
        "PATCH",
        &format!("/_admin/api/uploads/resumable/{id}?offset=16&final=1"),
        &patch_headers,
        &[17, 18, 19, 20, 21],
    )
    .await;
    assert_eq!(last.status, 200);
    assert_eq!(json(&last.body)["received"], 21);
    let complete = request(
        &http,
        &addr,
        "POST",
        &format!("/_admin/api/uploads/resumable/{id}/complete"),
        &auth,
        b"",
    )
    .await;
    assert_eq!(complete.status, 202);
    let job_id = json(&complete.body)["job_id"].as_str().unwrap().to_string();
    let mut state = String::new();
    for _ in 0..50 {
        let job = request(
            &http,
            &addr,
            "GET",
            &format!("/_admin/api/jobs/{job_id}"),
            &[("Cookie", session_cookie.as_str())],
            b"",
        )
        .await;
        state = json(&job.body)["state"]
            .as_str()
            .unwrap_or_default()
            .to_string();
        if state == "completed" || state == "cleaned" {
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    assert!(
        state == "completed" || state == "cleaned",
        "job state: {state}"
    );
    let content = request(
        &http,
        &addr,
        "GET",
        &format!("/_admin/api/objects/content?bucket={bucket}&key=folder%2Ffile.bin"),
        &[("Cookie", session_cookie.as_str())],
        b"",
    )
    .await;
    assert_eq!(content.status, 200);
    assert_eq!(content.body, (1u8..=21).collect::<Vec<_>>());

    let abort_begin = request(
        &http,
        &addr,
        "POST",
        "/_admin/api/uploads/resumable",
        &auth,
        format!(r#"{{"bucket":"{bucket}","key":"abort.bin"}}"#).as_bytes(),
    )
    .await;
    let abort_id = json(&abort_begin.body)["id"].as_str().unwrap().to_string();
    let abort = request(
        &http,
        &addr,
        "DELETE",
        &format!("/_admin/api/uploads/resumable/{abort_id}"),
        &auth,
        b"",
    )
    .await;
    assert_eq!(abort.status, 200);
    let aborted_job = request(
        &http,
        &addr,
        "GET",
        &format!("/_admin/api/jobs/{abort_id}"),
        &[("Cookie", session_cookie.as_str())],
        b"",
    )
    .await;
    assert_eq!(json(&aborted_job.body)["state"], "reception_failed");
    assert!(!temp.path().join("data/staging").join(&abort_id).exists());
    let _ = child.kill();
    let _ = child.wait();
}
