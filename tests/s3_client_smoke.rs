use assert_cmd::prelude::*;
use aws_config::BehaviorVersion;
use aws_credential_types::{Credentials, provider::SharedCredentialsProvider};
use aws_sdk_s3::{Client, config::Region, primitives::ByteStream};
use std::fs;
use std::io::{BufRead, BufReader};
use std::net::TcpListener;
use std::process::{Command, Stdio};
use tempfile::TempDir;

fn free_bind_addr() -> String {
    let listener = TcpListener::bind("127.0.0.1:0").expect("free port");
    listener.local_addr().expect("local addr").to_string()
}

fn command_for(tempdir: &TempDir, bind_addr: &str) -> Command {
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
    command.env("TELEGRAM_S3_BIND_ADDR", bind_addr);
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

async fn s3_client(endpoint_url: &str) -> Client {
    let shared_config = aws_config::from_env()
        .behavior_version(BehaviorVersion::latest())
        .endpoint_url(endpoint_url)
        .region(Region::new("us-east-1"))
        .credentials_provider(SharedCredentialsProvider::new(Credentials::new(
            "access-key",
            "secret-key",
            None,
            None,
            "test",
        )))
        .load()
        .await;

    let config = aws_sdk_s3::config::Builder::from(&shared_config)
        .force_path_style(true)
        .build();
    Client::from_conf(config)
}

#[tokio::test]
async fn s3_crud_list_and_range_smoke_test() {
    let tempdir = TempDir::new().expect("tempdir");
    let bind_addr = free_bind_addr();
    let endpoint_url = format!("http://{bind_addr}");

    let mut server_command = command_for(&tempdir, &bind_addr);
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

    let client = s3_client(&endpoint_url).await;
    let bucket = format!("phase4-{}", uuid::Uuid::new_v4().simple());
    let key = "folder/object.txt";
    let body = ByteStream::from_static(b"hello phase four");

    client
        .create_bucket()
        .bucket(&bucket)
        .send()
        .await
        .expect("create bucket");

    client
        .put_object()
        .bucket(&bucket)
        .key(key)
        .body(body)
        .send()
        .await
        .expect("put object");

    let head = client
        .head_object()
        .bucket(&bucket)
        .key(key)
        .send()
        .await
        .expect("head object");
    assert_eq!(head.content_length, Some(16));

    let get = client
        .get_object()
        .bucket(&bucket)
        .key(key)
        .send()
        .await
        .expect("get object");
    let bytes = get.body.collect().await.expect("collect").into_bytes();
    assert_eq!(bytes.as_ref(), b"hello phase four");

    let range = client
        .get_object()
        .bucket(&bucket)
        .key(key)
        .range("bytes=6-10")
        .send()
        .await
        .expect("range get");
    let range_bytes = range.body.collect().await.expect("collect").into_bytes();
    assert_eq!(range_bytes.as_ref(), b"phase");

    let listed = client
        .list_objects_v2()
        .bucket(&bucket)
        .send()
        .await
        .expect("list objects");
    let listed_keys: Vec<_> = listed
        .contents
        .unwrap_or_default()
        .into_iter()
        .filter_map(|object| object.key)
        .collect();
    assert_eq!(listed_keys, vec![key.to_string()]);

    client
        .delete_object()
        .bucket(&bucket)
        .key(key)
        .send()
        .await
        .expect("delete object");

    client
        .delete_bucket()
        .bucket(&bucket)
        .send()
        .await
        .expect("delete bucket");

    let _ = child.kill();
    let _ = child.wait();
}
