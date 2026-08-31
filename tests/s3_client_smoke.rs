use assert_cmd::prelude::*;
use aws_credential_types::Credentials;
use aws_sigv4::http_request::{
    PayloadChecksumKind, PercentEncodingMode, SignableBody, SignableRequest, SigningSettings,
    UriPathNormalizationMode, sign,
};
use aws_sigv4::sign::v4;
use bytes::Bytes;
use http::Request;
use http_body_util::{BodyExt, Full};
use hyper_util::client::legacy::{Client, connect::HttpConnector};
use hyper_util::rt::TokioExecutor;
use std::collections::HashMap;
use std::fs;
use std::io::{BufRead, BufReader};
use std::net::TcpListener;
use std::process::{Command, Stdio};
use std::time::SystemTime;
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
    command.env("TELEGRAM_ADMIN_BIND_ADDR", "127.0.0.1:0");
    command.env("TELEGRAM_TRANSPORT_RUNTIME", "mock");
    command
}

#[tokio::test]
async fn s3_crud_list_and_range_smoke_test() {
    let tempdir = TempDir::new().expect("tempdir");
    let bind_addr = free_bind_addr();

    let mut server_command = command_for(&tempdir, &bind_addr);
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

    let bucket = format!("phase4-{}", uuid::Uuid::new_v4().simple());
    let key = "folder/object.txt";
    let body = b"hello phase four";

    let create = signed_request(
        &tempdir,
        &bind_addr,
        "PUT",
        &format!("/{bucket}"),
        None,
        &[],
        &[],
    )
    .await;
    assert_eq!(create.status, 200, "create bucket");

    let put = signed_request(
        &tempdir,
        &bind_addr,
        "PUT",
        &format!("/{bucket}/{key}"),
        None,
        &[],
        body,
    )
    .await;
    assert_eq!(put.status, 200, "put object");

    let head = signed_request(
        &tempdir,
        &bind_addr,
        "HEAD",
        &format!("/{bucket}/{key}"),
        None,
        &[],
        &[],
    )
    .await;
    assert_eq!(head.status, 200, "head object");
    assert_eq!(
        head.headers
            .get("content-length")
            .and_then(|value| value.parse::<usize>().ok()),
        Some(body.len())
    );

    let head_precondition = signed_request(
        &tempdir,
        &bind_addr,
        "HEAD",
        &format!("/{bucket}/{key}"),
        None,
        &[("if-none-match", "*")],
        &[],
    )
    .await;
    assert!(head_precondition.status < 500);

    let get = signed_request(
        &tempdir,
        &bind_addr,
        "GET",
        &format!("/{bucket}/{key}"),
        None,
        &[],
        &[],
    )
    .await;
    assert_eq!(
        get.status,
        200,
        "get object: {}",
        String::from_utf8_lossy(&get.body)
    );
    assert_eq!(get.body, body);

    let range = signed_request(
        &tempdir,
        &bind_addr,
        "GET",
        &format!("/{bucket}/{key}"),
        None,
        &[("range", "bytes=6-10")],
        &[],
    )
    .await;
    assert_eq!(range.status, 206, "range get");
    assert_eq!(range.body, b"phase");

    let get_precondition = signed_request(
        &tempdir,
        &bind_addr,
        "GET",
        &format!("/{bucket}/{key}"),
        None,
        &[("if-match", "\"bogus\"")],
        &[],
    )
    .await;
    assert!(get_precondition.status < 500);

    let put_precondition = signed_request(
        &tempdir,
        &bind_addr,
        "PUT",
        &format!("/{bucket}/{key}"),
        None,
        &[("if-none-match", "*")],
        b"blocked",
    )
    .await;
    assert!(put_precondition.status < 500);

    let listed = signed_request(
        &tempdir,
        &bind_addr,
        "GET",
        &format!("/{bucket}"),
        Some("list-type=2"),
        &[],
        &[],
    )
    .await;
    assert_eq!(listed.status, 200, "list objects");
    assert!(
        String::from_utf8(listed.body)
            .expect("utf8 list body")
            .contains(key)
    );

    let delete_object = signed_request(
        &tempdir,
        &bind_addr,
        "DELETE",
        &format!("/{bucket}/{key}"),
        None,
        &[],
        &[],
    )
    .await;
    assert!(delete_object.status == 204 || delete_object.status == 200);

    let delete_bucket = signed_request(
        &tempdir,
        &bind_addr,
        "DELETE",
        &format!("/{bucket}"),
        None,
        &[],
        &[],
    )
    .await;
    assert!(delete_bucket.status == 204 || delete_bucket.status == 200);

    let _ = child.kill();
    let _ = child.wait();
}

struct HttpResponse {
    status: u16,
    headers: HashMap<String, String>,
    body: Vec<u8>,
}

async fn signed_request(
    _tempdir: &TempDir,
    addr: &str,
    method: &str,
    path: &str,
    query: Option<&str>,
    extra_headers: &[(&str, &str)],
    body: &[u8],
) -> HttpResponse {
    let request_target = match query {
        Some(query) => format!("{path}?{query}"),
        None => path.to_string(),
    };
    let request_uri = format!("http://{addr}{request_target}");
    let host_header = addr.to_string();
    let mut request = Request::builder()
        .method(method)
        .uri(&request_uri)
        .header("host", &host_header);
    for (name, value) in extra_headers {
        request = request.header(*name, *value);
    }
    let request = request
        .body(Full::new(Bytes::copy_from_slice(body)))
        .expect("build request");

    let identity = Credentials::new(
        "access-key",
        "secret-key",
        None,
        None,
        "telegram-s3-smoke-test",
    )
    .into();
    let mut signing_settings = SigningSettings::default();
    signing_settings.payload_checksum_kind = PayloadChecksumKind::XAmzSha256;
    signing_settings.percent_encoding_mode = PercentEncodingMode::Single;
    signing_settings.uri_path_normalization_mode = UriPathNormalizationMode::Disabled;
    let signing_params = v4::SigningParams::builder()
        .identity(&identity)
        .region("us-east-1")
        .name("s3")
        .time(SystemTime::now())
        .settings(signing_settings)
        .build()
        .expect("signing params")
        .into();
    let signable_request = SignableRequest::new(
        method,
        &request_uri,
        request
            .headers()
            .iter()
            .map(|(name, value)| (name.as_str(), value.to_str().expect("header utf8"))),
        SignableBody::Bytes(body),
    )
    .expect("signable request");
    let (signing_instructions, _signature) = sign(signable_request, &signing_params)
        .expect("sign request")
        .into_parts();
    let mut request = request;
    signing_instructions.apply_to_request_http1x(&mut request);

    let client: Client<HttpConnector, Full<Bytes>> =
        Client::builder(TokioExecutor::new()).build_http();
    let response = client.request(request).await.expect("send request");
    let status = response.status().as_u16();
    let mut headers = HashMap::new();
    for (name, value) in response.headers() {
        headers.insert(
            name.as_str().to_ascii_lowercase(),
            value.to_str().expect("header utf8").to_string(),
        );
    }
    let body = response
        .into_body()
        .collect()
        .await
        .expect("read response body")
        .to_bytes()
        .to_vec();

    HttpResponse {
        status,
        headers,
        body,
    }
}
