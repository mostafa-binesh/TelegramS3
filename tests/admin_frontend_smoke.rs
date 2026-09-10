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
use telegram_s3::metadata::{MetadataStore, TelegramBootstrapSettings};
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
    fs::write(assets_dir.join("app.js"), "export const marker = 'app';").expect("ui asset");
    fs::write(
        assets_dir.join("TelegramPanel-cafebabe.js"),
        "export const marker = 'telegram';",
    )
    .expect("ui asset");
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
    store
        .set_telegram_account_phone("+15551234567")
        .expect("telegram phone confirmation");
}

#[tokio::test]
async fn authenticated_admin_surface_serves_dashboard_and_session_lifecycle() {
    let tempdir = TempDir::new().expect("tempdir");
    seed_telegram_settings(&tempdir);

    // Pair the CLI seed the way production would: create an operator account in
    // the same metadata store the server will use, before it boots.
    let mut cli = Command::cargo_bin("telegram-s3").expect("binary");
    cli.env(
        "TELEGRAM_METADATA_PATH",
        tempdir.path().join("metadata.sqlite"),
    );
    cli.arg("users");
    cli.arg("create");
    cli.arg("admin");
    cli.args(["--password", "correct-horse-battery-staple"]);
    let cli_out = cli.output().expect("seed admin");
    assert!(
        cli_out.status.success(),
        "seed failed: {}",
        String::from_utf8_lossy(&cli_out.stderr)
    );

    let bind_addr = free_bind_addr();

    let mut server_command = command_for(&tempdir, &bind_addr);

    // Seeded before boot on purpose: the recovery snapshot is only rescanned at startup
    // and then once a minute, so an orphan created after boot would not show up in the
    // cached snapshot the acknowledge endpoint validates against.
    let ack_staging = tempdir
        .path()
        .join("data")
        .join("staging")
        .join("orphaned-acknowledge");
    fs::create_dir_all(&ack_staging).expect("ack staging dir");
    fs::write(ack_staging.join("note.txt"), "orphaned").expect("ack staging file");

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

    for attempt in 0..7 {
        let failed_login = http_request(
            &client,
            &bind_addr,
            "POST",
            "/_admin/api/session/login",
            &[],
            br#"{"username":"admin","password":"wrong-password"}"#,
        )
        .await;
        assert_eq!(
            failed_login.status, 401,
            "failed login attempt {attempt} should stay unauthorized"
        );
        assert!(failed_login.body.contains("invalid username or password"));
    }

    let login = http_request(
        &client,
        &bind_addr,
        "POST",
        "/_admin/api/session/login",
        &[],
        br#"{"username":"admin","password":"correct-horse-battery-staple"}"#,
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

    let telegram_settings = http_request(
        &client,
        &bind_addr,
        "GET",
        "/_admin/api/telegram/settings",
        &[("Cookie", cookie_header.as_str())],
        b"",
    )
    .await;
    assert_eq!(telegram_settings.status, 200);
    assert!(telegram_settings.body.contains("+15551234567"));
    assert!(
        !telegram_settings
            .body
            .contains("telegram_account_phone_hash")
    );

    let post_success_failed_login = http_request(
        &client,
        &bind_addr,
        "POST",
        "/_admin/api/session/login",
        &[],
        br#"{"username":"admin","password":"wrong-password"}"#,
    )
    .await;
    assert_eq!(post_success_failed_login.status, 401);
    assert!(
        post_success_failed_login
            .body
            .contains("invalid username or password")
    );

    let post_success_failed_login_2 = http_request(
        &client,
        &bind_addr,
        "POST",
        "/_admin/api/session/login",
        &[],
        br#"{"username":"admin","password":"wrong-password"}"#,
    )
    .await;
    assert_eq!(
        post_success_failed_login_2.status, 401,
        "successful login should reset the limiter window"
    );
    assert!(
        post_success_failed_login_2
            .body
            .contains("invalid username or password")
    );

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
    assert!(!overview.body.contains("\"endpoint\""));
    assert!(overview.body.contains("\"checks\""));

    // The startup scan runs concurrently with the listener, so give it a moment to land.
    let mut recovery = Value::Null;
    for _ in 0..40 {
        let body = http_request(
            &client,
            &bind_addr,
            "GET",
            "/_admin/api/overview",
            &[("Cookie", cookie_header.as_str())],
            b"",
        )
        .await
        .body;
        let parsed: Value = serde_json::from_str(&body).expect("overview json");
        let candidate = parsed["recovery"].clone();
        if candidate["issues"]
            .as_array()
            .is_some_and(|a| !a.is_empty())
        {
            recovery = candidate;
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(250)).await;
    }
    let issues = recovery["issues"]
        .as_array()
        .expect("recovery issues in overview");
    let total = recovery["issue_count"].as_u64().expect("issue_count");
    assert_eq!(
        recovery["unacknowledged_count"].as_u64(),
        Some(total),
        "nothing is acknowledged yet"
    );
    let issue_id = issues[0]["id"].as_str().expect("issue id").to_string();

    let unknown = http_request(
        &client,
        &bind_addr,
        "POST",
        "/_admin/api/recovery/acknowledge",
        &[
            ("Cookie", cookie_header.as_str()),
            ("X-CSRF-Token", csrf.as_str()),
        ],
        br#"{"ids":["deadbeefdeadbeef"]}"#,
    )
    .await;
    assert_eq!(unknown.status, 404, "unknown fingerprints are rejected");

    let acknowledge = http_request(
        &client,
        &bind_addr,
        "POST",
        "/_admin/api/recovery/acknowledge",
        &[
            ("Cookie", cookie_header.as_str()),
            ("X-CSRF-Token", csrf.as_str()),
        ],
        format!(r#"{{"ids":["{issue_id}"]}}"#).as_bytes(),
    )
    .await;
    assert_eq!(acknowledge.status, 200);
    assert!(acknowledge.body.contains("\"ok\":true"));

    let after_ack = read_recovery(&client, &bind_addr, &cookie_header).await;
    assert_eq!(
        after_ack["unacknowledged_count"].as_u64(),
        Some(total - 1),
        "acknowledged issues drop out of the overview count"
    );
    assert_eq!(
        after_ack["issue_count"].as_u64(),
        Some(total),
        "the issue itself stays visible"
    );
    let acked = find_issue(&after_ack, &issue_id).expect("acknowledged issue still listed");
    assert!(acked["acknowledged_at"].is_string());
    assert_eq!(acked["acknowledged_by"].as_str(), Some("admin"));

    let unacknowledge = http_request(
        &client,
        &bind_addr,
        "POST",
        "/_admin/api/recovery/unacknowledge",
        &[
            ("Cookie", cookie_header.as_str()),
            ("X-CSRF-Token", csrf.as_str()),
        ],
        format!(r#"{{"ids":["{issue_id}"]}}"#).as_bytes(),
    )
    .await;
    assert_eq!(unacknowledge.status, 200);

    let after_restore = read_recovery(&client, &bind_addr, &cookie_header).await;
    assert_eq!(
        after_restore["unacknowledged_count"].as_u64(),
        Some(total),
        "restoring puts the issue back in the count"
    );
    let restored = find_issue(&after_restore, &issue_id).expect("restored issue listed");
    assert!(restored["acknowledged_at"].is_null());

    let orphaned_staging = tempdir
        .path()
        .join("data")
        .join("staging")
        .join("orphaned-repair");
    fs::create_dir_all(&orphaned_staging).expect("orphaned staging dir");
    fs::write(orphaned_staging.join("note.txt"), "orphaned").expect("orphaned staging file");

    let repair = http_request(
        &client,
        &bind_addr,
        "POST",
        "/_admin/api/recovery/repair",
        &[
            ("Cookie", cookie_header.as_str()),
            ("X-CSRF-Token", csrf.as_str()),
        ],
        b"",
    )
    .await;
    assert_eq!(repair.status, 200);
    assert!(repair.body.contains("\"ok\":true"));
    assert!(repair.body.contains("\"report\""));
    assert!(repair.body.contains("\"quarantined_objects\""));

    let users = http_request(
        &client,
        &bind_addr,
        "GET",
        "/_admin/api/users",
        &[("Cookie", cookie_header.as_str())],
        b"",
    )
    .await;
    assert_eq!(users.status, 200);
    assert!(users.body.contains("\"admin\""));

    let create_bucket = http_request(
        &client,
        &bind_addr,
        "POST",
        "/_admin/api/buckets",
        &[
            ("Cookie", cookie_header.as_str()),
            ("X-CSRF-Token", csrf.as_str()),
        ],
        br#"{"name":"ui-created-\u0641\u0627\u06cc\u0644"}"#,
    )
    .await;
    assert_eq!(create_bucket.status, 201);
    assert!(create_bucket.body.contains("\"name\":\"ui-created-فایل\""));

    let buckets = http_request(
        &client,
        &bind_addr,
        "GET",
        "/_admin/api/buckets",
        &[("Cookie", cookie_header.as_str())],
        b"",
    )
    .await;
    assert_eq!(buckets.status, 200);
    assert!(buckets.body.contains("\"ui-created-فایل\""));

    let delete_bucket = http_request(
        &client,
        &bind_addr,
        "DELETE",
        "/_admin/api/buckets/ui-created-%D9%81%D8%A7%DB%8C%D9%84",
        &[
            ("Cookie", cookie_header.as_str()),
            ("X-CSRF-Token", csrf.as_str()),
        ],
        b"",
    )
    .await;
    assert_eq!(delete_bucket.status, 200);

    let wrong_phone_disconnect = http_request(
        &client,
        &bind_addr,
        "POST",
        "/_admin/api/telegram/disconnect",
        &[
            ("Cookie", cookie_header.as_str()),
            ("X-CSRF-Token", csrf.as_str()),
        ],
        br#"{"delete_uploaded_files":false,"phone_confirmation":"+15550000000"}"#,
    )
    .await;
    assert_eq!(wrong_phone_disconnect.status, 403);

    let disconnect = http_request(
        &client,
        &bind_addr,
        "POST",
        "/_admin/api/telegram/disconnect",
        &[
            ("Cookie", cookie_header.as_str()),
            ("X-CSRF-Token", csrf.as_str()),
        ],
        br#"{"delete_uploaded_files":false,"phone_confirmation":"+15551234567"}"#,
    )
    .await;
    assert_eq!(disconnect.status, 202);
    assert!(
        disconnect
            .body
            .contains("uploaded Telegram files were left")
    );

    let mut removed_overview = Value::Null;
    for _ in 0..20 {
        let response = http_request(
            &client,
            &bind_addr,
            "GET",
            "/_admin/api/overview",
            &[("Cookie", cookie_header.as_str())],
            b"",
        )
        .await;
        assert_eq!(response.status, 200);
        removed_overview = serde_json::from_str(&response.body).expect("overview json");
        if removed_overview["storage"]["buckets"] == 0
            && removed_overview["telegram"]["connection_state"] == "not_configured"
        {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }
    assert_eq!(removed_overview["storage"]["buckets"], 0);
    assert_eq!(
        removed_overview["telegram"]["connection_state"],
        "not_configured"
    );

    // Unauthenticated access to the management API must be rejected.
    let unauth = http_request(&client, &bind_addr, "GET", "/_admin/api/users", &[], b"").await;
    assert_eq!(unauth.status, 401);

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

    let asset = http_request(
        &client,
        &bind_addr,
        "GET",
        "/_admin/assets/app.css",
        &[],
        b"",
    )
    .await;
    assert_eq!(asset.status, 200);
    assert_eq!(asset.body, "body{}");

    let telegram_chunk = http_request(
        &client,
        &bind_addr,
        "GET",
        "/_admin/assets/TelegramPanel-cafebabe.js",
        &[],
        b"",
    )
    .await;
    assert_eq!(telegram_chunk.status, 200);
    assert_eq!(telegram_chunk.body, "export const marker = 'telegram';");

    let missing_asset = http_request(
        &client,
        &bind_addr,
        "GET",
        "/_admin/assets/missing.js",
        &[],
        b"",
    )
    .await;
    assert_eq!(missing_asset.status, 404);

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

/// Re-reads `/overview` and hands back just the `recovery` block.
async fn read_recovery(
    client: &Client<HttpConnector, Full<Bytes>>,
    addr: &str,
    cookie_header: &str,
) -> Value {
    let response = http_request(
        client,
        addr,
        "GET",
        "/_admin/api/overview",
        &[("Cookie", cookie_header)],
        b"",
    )
    .await;
    assert_eq!(response.status, 200);
    let parsed: Value = serde_json::from_str(&response.body).expect("overview json");
    parsed["recovery"].clone()
}

fn find_issue<'a>(recovery: &'a Value, id: &str) -> Option<&'a Value> {
    recovery["issues"]
        .as_array()?
        .iter()
        .find(|issue| issue["id"].as_str() == Some(id))
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
