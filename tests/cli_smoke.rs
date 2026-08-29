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
    assert!(migrate_stdout.contains("database migrated to schema version 1"));

    let doctor_stdout = run_and_capture(&tempdir, &["doctor"]);
    assert!(doctor_stdout.contains("metadata schema version: 1"));

    let rebuild_stdout = run_and_capture(&tempdir, &["index", "rebuild"]);
    assert!(rebuild_stdout.contains("rebuild complete"));

    let verify_stdout = run_and_capture(&tempdir, &["index", "verify"]);
    assert!(verify_stdout.contains("mismatched rows: 0"));

    let status_stdout = run_and_capture(&tempdir, &["db", "status"]);
    assert!(status_stdout.contains("schema version: 1"));
}
