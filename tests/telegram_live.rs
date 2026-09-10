//! Explicit live-Telegram acceptance drill.
//!
//! This test is ignored by default. It requires an isolated metadata/data
//! directory and Telegram bootstrap settings supplied through the environment;
//! it never reads the repository's ignored `data/` tree.

use std::time::Duration;
use telegram_s3::AppConfig;
use telegram_s3::object_format::ObjectFormatService;
use uuid::Uuid;

#[tokio::test]
#[ignore = "requires TELEGRAM_LIVE_TESTS=1 and isolated live Telegram settings"]
async fn isolated_live_roundtrip_range_and_tombstone() {
    assert_eq!(
        std::env::var("TELEGRAM_LIVE_TESTS").as_deref(),
        Ok("1"),
        "set TELEGRAM_LIVE_TESTS=1 explicitly before running the live drill"
    );
    let config = AppConfig::from_env();
    config.validate().expect("live test configuration");
    assert!(
        config
            .telegram_metadata_path
            .as_deref()
            .is_some_and(|path| path.contains("live-test")),
        "live tests must use a dedicated metadata path containing live-test"
    );
    assert!(
        config
            .telegram_data_dir
            .as_deref()
            .is_some_and(|path| path.contains("live-test")),
        "live tests must use a dedicated data directory containing live-test"
    );

    let service = ObjectFormatService::open(&config).await.expect("service");
    let bucket = format!("live-{}", Uuid::new_v4().simple());
    let key = "release-drill/payload.bin";
    let payload = b"telegram-s3 live release drill payload";
    service.create_bucket(&bucket).expect("bucket");
    let manifest = service
        .put_bytes(&bucket, key, "application/octet-stream", payload)
        .await
        .expect("live put");
    assert_eq!(
        service
            .read_bytes(&bucket, key, 0..payload.len() as u64)
            .await
            .expect("live read"),
        payload
    );
    assert_eq!(
        service
            .read_bytes(&bucket, key, 8..16)
            .await
            .expect("live range"),
        &payload[8..16]
    );

    service
        .delete_object(&bucket, key, None, None, None)
        .expect("tombstone");
    service.ensure_workers();
    for _ in 0..120 {
        if service
            .get_active_manifest(&bucket, key)
            .expect("active manifest")
            .is_none()
        {
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    assert!(
        service
            .get_active_manifest(&bucket, key)
            .expect("active manifest")
            .is_none()
    );
    let _ = service.garbage_collect(false, time::Duration::ZERO).await;
    let _ = service.delete_bucket(&bucket);
    service.shutdown_workers().await;
    assert!(manifest.object_id != Uuid::nil());
}
