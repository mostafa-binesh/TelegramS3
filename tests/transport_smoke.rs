use assert_cmd::prelude::*;
use std::fs;
use std::io::{BufRead, BufReader};
use std::process::{Command, Stdio};
use tempfile::TempDir;

fn command_for(tempdir: &TempDir) -> Command {
    let metadata_path = tempdir.path().join("metadata.sqlite");
    let session_path = tempdir.path().join("telegram.session");
    let data_dir = tempdir.path().join("data");
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
    command.env("TELEGRAM_PROXY_MODE", "auto");
    command.env("TELEGRAM_FLOOD_WAIT_RESPECT", "true");
    command.env("TELEGRAM_CHUNK_SIZE", "1048576");
    command.env("TELEGRAM_CONNECTION_TIMEOUT_SECS", "30");
    command.env("TELEGRAM_REQUEST_TIMEOUT_SECS", "30");
    command.env("TELEGRAM_TRANSFER_TIMEOUT_SECS", "900");
    command.env("TELEGRAM_RETRY_COUNT", "5");
    command.env("TELEGRAM_RETRY_BACKOFF_MS", "500");
    command.env("TELEGRAM_TRANSPORT_RUNTIME", "mock");
    command
}

#[test]
fn auth_status_logout_doctor_and_server_bootstrap_smoke_test() {
    let tempdir = TempDir::new().expect("tempdir");

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
    let reader = BufReader::new(stdout);
    let mut saw_listening = false;
    for line in reader.lines().take(50) {
        let line = line.expect("server line");
        if line.contains("listening on") {
            saw_listening = true;
            break;
        }
    }
    assert!(saw_listening);
    let _ = child.kill();
    let _ = child.wait();
}
