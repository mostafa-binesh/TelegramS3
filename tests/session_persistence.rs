use assert_cmd::prelude::*;
use std::fs;
use std::process::Command;
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
fn session_file_survives_process_restart() {
    let tempdir = TempDir::new().expect("tempdir");
    let session_path = tempdir.path().join("telegram.session");

    let first = command_for(&tempdir)
        .args(["auth", "status"])
        .assert()
        .success();
    let first_stdout = String::from_utf8(first.get_output().stdout.clone()).expect("utf8");
    assert!(first_stdout.contains("auth status: session state"));
    assert!(session_path.exists());

    let second = command_for(&tempdir)
        .args(["auth", "status"])
        .assert()
        .success();
    let second_stdout = String::from_utf8(second.get_output().stdout.clone()).expect("utf8");
    assert!(second_stdout.contains("auth status: session state"));
    assert!(session_path.exists());
}
