use assert_cmd::prelude::*;
use std::fs;
use std::io::{BufRead, BufReader};
use std::path::PathBuf;
use std::process::{Command, Stdio};
use telegram_s3::metadata::{MetadataStore, TelegramBootstrapSettings};
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
    let data_dir = tempdir.path().join("data");
    let ui_dir = prepare_admin_ui(tempdir);
    fs::create_dir_all(&data_dir).expect("data dir");

    let mut command = Command::cargo_bin("telegram-s3").expect("binary");
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
            telegram_session_path: Some(
                tempdir
                    .path()
                    .join("telegram.session")
                    .display()
                    .to_string(),
            ),
            telegram_storage_chat_id: Some("-1001234567890".to_string()),
            telegram_proxy_mode: Some("auto".to_string()),
            ..TelegramBootstrapSettings::default()
        })
        .expect("telegram settings");
}

#[test]
fn auth_status_logout_doctor_and_server_bootstrap_smoke_test() {
    let tempdir = TempDir::new().expect("tempdir");
    seed_telegram_settings(&tempdir);

    let status = command_for(&tempdir)
        .args(["auth", "status"])
        .assert()
        .success();
    let status_stdout = String::from_utf8(status.get_output().stdout.clone()).expect("utf8");
    assert!(status_stdout.contains("auth status: session state"));

    let logout = command_for(&tempdir)
        .args(["auth", "logout"])
        .assert()
        .success();
    let logout_stdout = String::from_utf8(logout.get_output().stdout.clone()).expect("utf8");
    assert!(logout_stdout.contains("logout complete"));

    let doctor = command_for(&tempdir).arg("doctor").assert().success();
    let doctor_stdout = String::from_utf8(doctor.get_output().stdout.clone()).expect("utf8");
    assert!(doctor_stdout.contains("s3 bind address: 127.0.0.1:9000"));
    assert!(doctor_stdout.contains("telegram bootstrap:"));

    let mut server_command = command_for(&tempdir);
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
        let line = line.trim_end_matches(['\r', '\n']);
        if line.starts_with("listening on ") {
            saw_listening = true;
            break;
        }
    }
    assert!(saw_listening);
    let _ = child.kill();
    let _ = child.wait();
}
