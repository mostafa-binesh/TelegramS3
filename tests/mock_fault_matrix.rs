//! Deterministic mock transport outcome matrix.
//!
//! This is intentionally one test because the scripted outcome is process
//! environment state. Each scenario uses a fresh worker lifecycle and a unique
//! object key.

use std::time::Duration;
use telegram_s3::AppConfig;
use telegram_s3::MetadataStore;
use telegram_s3::TelegramBootstrapSettings;
use telegram_s3::object_format::ObjectFormatService;
use tempfile::TempDir;
use tokio::time::timeout;

fn config(tempdir: &TempDir) -> AppConfig {
    AppConfig {
        telegram_metadata_path: Some(tempdir.path().join("metadata.sqlite").display().to_string()),
        telegram_data_dir: Some(tempdir.path().join("data").display().to_string()),
        telegram_s3_master_key: Some("matrix-master-key".into()),
        rustfs_access_key: Some("matrix-access-key".into()),
        rustfs_secret_key: Some("matrix-secret-key".into()),
        telegram_admin_bootstrap_secret: Some("matrix-admin-secret".into()),
        ..AppConfig::default()
    }
}

fn seed(tempdir: &TempDir) {
    let store = MetadataStore::open(tempdir.path().join("metadata.sqlite")).expect("metadata");
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

#[tokio::test]
async fn scripted_mock_transport_outcomes_are_deterministic_and_recovery_safe() {
    unsafe {
        std::env::set_var("TELEGRAM_TRANSPORT_RUNTIME", "mock");
    }
    let tempdir = TempDir::new().expect("tempdir");
    seed(&tempdir);
    let service = ObjectFormatService::open(&config(&tempdir))
        .await
        .expect("service");
    service.create_bucket("fault-matrix").expect("bucket");

    for fault in [
        "timeout",
        "proxy_disconnect",
        "flood_wait",
        "auth_key_unregistered",
        "peer_lookup",
        "missing",
        "ambiguous",
        "byte_collision",
    ] {
        unsafe {
            std::env::set_var("TELEGRAM_MOCK_FAULT", fault);
        }
        let key = format!("faults/{fault}.bin");
        let result = timeout(
            Duration::from_secs(5),
            service.put_bytes(
                "fault-matrix",
                &key,
                "application/octet-stream",
                b"fault matrix payload",
            ),
        )
        .await
        .expect("fault scenario must not hang");
        if fault == "ambiguous" {
            assert!(
                result.is_ok(),
                "an ambiguous send with an exact remote match should self-heal"
            );
        } else {
            assert!(
                result.is_err(),
                "{fault} should fail the remote publication"
            );
            let error = result.expect_err("checked above").to_string();
            assert!(
                error.contains("scripted") || error.contains("acknowledgement is unknown"),
                "{fault} error should preserve a recovery diagnostic: {error}"
            );
        }
        service.shutdown_workers().await;
        unsafe {
            std::env::remove_var("TELEGRAM_MOCK_FAULT");
        }
    }
    unsafe {
        std::env::remove_var("TELEGRAM_TRANSPORT_RUNTIME");
    }
}
