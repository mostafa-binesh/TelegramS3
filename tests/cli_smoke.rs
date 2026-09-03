use assert_cmd::prelude::*;
use std::fs;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpStream;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use tempfile::TempDir;

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

fn command_for(tempdir: &TempDir) -> Command {
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

fn run_and_capture(tempdir: &TempDir, args: &[&str]) -> String {
    let assert = command_for(tempdir).args(args).assert().success();
    String::from_utf8(assert.get_output().stdout.clone()).expect("utf8 stdout")
}

#[test]
fn config_doctor_db_and_index_smoke_test() {
    let tempdir = TempDir::new().expect("tempdir");

    let config_stdout = run_and_capture(&tempdir, &["config", "check"]);
    assert!(config_stdout.contains("configuration looks structurally valid"));

    let migrate_stdout = run_and_capture(&tempdir, &["db", "migrate"]);
    assert!(contains_schema_version(
        &migrate_stdout,
        "database migrated to schema version"
    ));

    let doctor_stdout = run_and_capture(&tempdir, &["doctor"]);
    assert!(contains_schema_version(
        &doctor_stdout,
        "metadata schema version: "
    ));

    let rebuild_stdout = run_and_capture(&tempdir, &["index", "rebuild"]);
    assert!(rebuild_stdout.contains("rebuild complete"));

    let verify_stdout = run_and_capture(&tempdir, &["index", "verify"]);
    assert!(verify_stdout.contains("mismatched rows: 0"));

    let status_stdout = run_and_capture(&tempdir, &["db", "status"]);
    assert!(contains_schema_version(&status_stdout, "schema version: "));

    let repair_dry_run = run_and_capture(&tempdir, &["repair", "--dry-run"]);
    assert!(repair_dry_run.contains("repair dry-run"));

    let repair_stdout = run_and_capture(&tempdir, &["repair"]);
    assert!(repair_stdout.contains("repair complete"));

    let gc_dry_run = run_and_capture(&tempdir, &["gc", "--dry-run"]);
    assert!(gc_dry_run.contains("gc dry-run"));

    let mut server_command = command_for(&tempdir);
    server_command.arg("server");
    server_command.stdout(Stdio::piped());
    let mut child = server_command.spawn().expect("spawn server");
    let stdout = child.stdout.take().expect("server stdout");
    let mut reader = BufReader::new(stdout);
    let mut saw_listening = false;
    let mut admin_addr = None;
    let mut line = String::new();
    for _ in 0..50 {
        line.clear();
        let bytes = reader.read_line(&mut line).expect("server line");
        if bytes == 0 {
            break;
        }
        let line = line.trim_end_matches(['\r', '\n']);
        if let Some(address) = line.strip_prefix("admin listening on ") {
            admin_addr = Some(address.trim().to_string());
        }
        if line.starts_with("listening on ") {
            saw_listening = true;
        }
        if saw_listening && admin_addr.is_some() {
            break;
        }
    }
    assert!(saw_listening);

    let admin_addr = admin_addr.expect("admin addr");
    let healthz = http_get(&admin_addr, "/healthz");
    assert!(healthz.contains("ok"));
    let metrics = http_get(&admin_addr, "/metrics");
    assert!(metrics.contains("telegram_s3_bootstrap_ok 1"));
    assert!(metrics.contains("telegram_s3_metadata_committed_objects"));

    let _ = child.kill();
    let _ = child.wait();
}

fn contains_schema_version(output: &str, prefix: &str) -> bool {
    output.lines().any(|line| {
        line.find(prefix).is_some_and(|index| {
            line[index + prefix.len()..]
                .trim_start()
                .chars()
                .next()
                .is_some_and(|c| c.is_ascii_digit())
        })
    })
}

fn http_get(addr: &str, path: &str) -> String {
    let mut stream = TcpStream::connect(addr).expect("connect admin");
    let request = format!("GET {path} HTTP/1.1\r\nHost: {addr}\r\nConnection: close\r\n\r\n");
    stream.write_all(request.as_bytes()).expect("write request");
    let mut response = String::new();
    stream.read_to_string(&mut response).expect("read response");
    response
}
