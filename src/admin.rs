use crate::config::AppConfig;
use crate::metadata::MetadataStatus;
use crate::object_format::{ObjectFormatService, ObjectFormatStatus};
use crate::redact::{redact_path, redact_phone_number};
use crate::telegram::{SessionState, TelegramTransportStatus};
use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use bytes::Bytes;
use http::header::{self, HeaderValue};
use http::{Method, StatusCode};
use http_body_util::BodyExt;
use hyper::body::Incoming;
use hyper::{Request, Response};
use ring::hmac;
use s3s::Body;
use serde::{Deserialize, Serialize};
use std::net::SocketAddr;
use std::path::{Component, Path, PathBuf};
use std::sync::Arc;
use time::{Duration, OffsetDateTime};
use tokio::fs;
use uuid::Uuid;

const ADMIN_ROUTE_PREFIX: &str = "/_admin";
const ADMIN_API_PREFIX: &str = "/_admin/api/";
const ADMIN_ASSET_PREFIX: &str = "/_admin/assets/";
const ADMIN_SESSION_COOKIE: &str = "telegram_s3_admin_session";
const ADMIN_COOKIE_PATH: &str = "/_admin";
const ADMIN_SESSION_TTL_SECONDS: i64 = 12 * 60 * 60;
const ADMIN_CSRF_HEADER: &str = "x-csrf-token";

#[derive(Clone)]
pub struct AdminUiState {
    config: AppConfig,
    object_format: Arc<ObjectFormatService>,
    transport_status: TelegramTransportStatus,
    s3_addr: SocketAddr,
    admin_addr: SocketAddr,
    bootstrap_secret: String,
    ui_dist_dir: PathBuf,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "snake_case")]
struct SessionSnapshot {
    authenticated: bool,
    issued_at: String,
    expires_at: String,
    csrf_token: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "snake_case")]
struct SessionResponse {
    authenticated: bool,
    issued_at: Option<String>,
    expires_at: Option<String>,
    csrf_token: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "snake_case")]
struct EndpointResponse {
    s3_bind_addr: String,
    admin_bind_addr: String,
    admin_route_prefix: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "snake_case")]
struct StorageResponse {
    metadata_path: String,
    data_dir: String,
    session_path: String,
    buckets: u64,
    committed_objects: u64,
    active_objects: u64,
    staged_objects: u64,
    recovery_markers: u64,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "snake_case")]
struct CapacityResponse {
    chunk_size: u64,
    recovery_required_objects: u64,
    orphaned_chunks: u64,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "snake_case")]
struct TelegramResponse {
    session_state: String,
    proxy_kind: String,
    proxy_url: Option<String>,
    phone_number: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "snake_case")]
struct CheckItem {
    label: String,
    ok: bool,
    detail: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "snake_case")]
struct BootstrapResponse {
    ready: bool,
    authenticated: bool,
    session_state: String,
    phone_number: Option<String>,
    proxy_mode: String,
    proxy_url: Option<String>,
    checks: Vec<CheckItem>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "snake_case")]
struct OverviewResponse {
    checked_at: String,
    session: SessionSnapshot,
    endpoint: EndpointResponse,
    storage: StorageResponse,
    capacity: CapacityResponse,
    telegram: TelegramResponse,
    bootstrap: BootstrapResponse,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "snake_case")]
struct LoginRequest {
    bootstrap_secret: String,
}

#[derive(Debug, Deserialize, Serialize)]
struct SessionClaimsWire {
    iat: i64,
    exp: i64,
    csrf: String,
}

#[derive(Debug, Clone)]
struct IssuedSession {
    snapshot: SessionSnapshot,
    cookie_value: String,
}

type AdminError = Box<Response<Body>>;

impl AdminUiState {
    pub fn new(
        config: AppConfig,
        object_format: Arc<ObjectFormatService>,
        transport_status: TelegramTransportStatus,
        s3_addr: SocketAddr,
        admin_addr: SocketAddr,
        bootstrap_secret: String,
        ui_dist_dir: PathBuf,
    ) -> Self {
        Self {
            config,
            object_format,
            transport_status,
            s3_addr,
            admin_addr,
            bootstrap_secret,
            ui_dist_dir,
        }
    }

    pub fn ui_dist_dir(&self) -> &Path {
        &self.ui_dist_dir
    }

    pub fn is_admin_route(path: &str) -> bool {
        path == ADMIN_ROUTE_PREFIX
            || path.starts_with(ADMIN_API_PREFIX)
            || path.starts_with(ADMIN_ASSET_PREFIX)
            || path.starts_with("/_admin/")
    }

    pub async fn handle_request(self: Arc<Self>, request: Request<Incoming>) -> Response<Body> {
        let path = request.uri().path().to_string();
        if path.starts_with(ADMIN_API_PREFIX) {
            return self.handle_api_request(request).await;
        }

        self.handle_static_request(request).await
    }

    async fn handle_api_request(self: Arc<Self>, request: Request<Incoming>) -> Response<Body> {
        let method = request.method().clone();
        let path = request.uri().path().to_string();
        let session = read_session_from_request(&request, &self.bootstrap_secret);
        match (method, path.as_str()) {
            (Method::GET, "/_admin/api/session") => {
                json_response(StatusCode::OK, SessionResponse::from(session.as_ref()))
            }
            (Method::POST, "/_admin/api/session/login") => {
                let login = match read_json::<LoginRequest>(request).await {
                    Ok(login) => login,
                    Err(response) => return *response,
                };
                if !constant_time_eq(
                    login.bootstrap_secret.as_bytes(),
                    self.bootstrap_secret.as_bytes(),
                ) {
                    return json_error(StatusCode::UNAUTHORIZED, "invalid bootstrap secret");
                }
                let issued = match issue_session(self.bootstrap_secret.as_bytes()) {
                    Ok(issued) => issued,
                    Err(response) => return *response,
                };
                with_set_cookie(
                    json_response(
                        StatusCode::OK,
                        SessionResponse::from(Some(&issued.snapshot)),
                    ),
                    issued.cookie_value,
                )
            }
            (Method::POST, "/_admin/api/session/logout") => {
                let Some(session) = session else {
                    return json_error(StatusCode::UNAUTHORIZED, "not authenticated");
                };
                if !require_csrf(&request, &session) {
                    return json_error(StatusCode::FORBIDDEN, "invalid csrf token");
                }
                let response = json_response(
                    StatusCode::OK,
                    SessionResponse {
                        authenticated: false,
                        issued_at: None,
                        expires_at: None,
                        csrf_token: None,
                    },
                );
                with_clear_cookie(response)
            }
            (Method::POST, "/_admin/api/session/refresh") => {
                let Some(session) = session else {
                    return json_error(StatusCode::UNAUTHORIZED, "not authenticated");
                };
                if !require_csrf(&request, &session) {
                    return json_error(StatusCode::FORBIDDEN, "invalid csrf token");
                }
                let issued = match issue_session(self.bootstrap_secret.as_bytes()) {
                    Ok(issued) => issued,
                    Err(response) => return *response,
                };
                with_set_cookie(
                    json_response(
                        StatusCode::OK,
                        SessionResponse::from(Some(&issued.snapshot)),
                    ),
                    issued.cookie_value,
                )
            }
            (Method::GET, "/_admin/api/overview") => {
                let Some(session) = session else {
                    return json_error(StatusCode::UNAUTHORIZED, "not authenticated");
                };
                json_response(StatusCode::OK, self.build_overview(&session))
            }
            (Method::GET, "/_admin/api/bootstrap-status") => json_response(
                StatusCode::OK,
                self.build_bootstrap_status(session.as_ref()),
            ),
            (Method::POST, "/_admin/api/onboarding/check") => {
                let Some(session) = session else {
                    return json_error(StatusCode::UNAUTHORIZED, "not authenticated");
                };
                json_response(StatusCode::OK, self.build_bootstrap_status(Some(&session)))
            }
            _ => json_error(StatusCode::NOT_FOUND, "not found"),
        }
    }

    async fn handle_static_request(self: Arc<Self>, request: Request<Incoming>) -> Response<Body> {
        if !matches!(request.method(), &Method::GET | &Method::HEAD) {
            return json_error(StatusCode::METHOD_NOT_ALLOWED, "method not allowed");
        }

        let path = request.uri().path();
        let asset_path = path.strip_prefix(ADMIN_ASSET_PREFIX);
        if let Some(asset_path) = asset_path {
            let Some(file_path) = safe_join(self.ui_dist_dir(), asset_path) else {
                return json_error(StatusCode::NOT_FOUND, "asset not found");
            };
            return match read_static_file(&file_path).await {
                Ok(Some(response)) => response,
                Ok(None) => {
                    match read_static_asset_fallback(self.ui_dist_dir(), asset_path).await {
                        Ok(Some(response)) => response,
                        Ok(None) => json_error(StatusCode::NOT_FOUND, "asset not found"),
                        Err(_) => json_error(
                            StatusCode::SERVICE_UNAVAILABLE,
                            "admin ui assets unavailable",
                        ),
                    }
                }
                Err(_) => json_error(
                    StatusCode::SERVICE_UNAVAILABLE,
                    "admin ui assets unavailable",
                ),
            };
        }

        if path == ADMIN_ROUTE_PREFIX || path == "/_admin/" || path.starts_with("/_admin/") {
            let index = self.ui_dist_dir().join("index.html");
            return match read_static_file(&index).await {
                Ok(Some(response)) => response,
                Ok(None) => json_error(StatusCode::SERVICE_UNAVAILABLE, "admin ui assets missing"),
                Err(_) => json_error(
                    StatusCode::SERVICE_UNAVAILABLE,
                    "admin ui assets unavailable",
                ),
            };
        }

        json_error(StatusCode::NOT_FOUND, "not found")
    }

    fn build_overview(&self, session: &SessionSnapshot) -> OverviewResponse {
        let metadata = self
            .object_format
            .metadata_status()
            .unwrap_or_else(|_| empty_metadata_status());
        let object = self
            .object_format
            .status()
            .unwrap_or_else(|_| empty_object_status());
        OverviewResponse {
            checked_at: rfc3339(OffsetDateTime::now_utc()),
            session: session.clone(),
            endpoint: EndpointResponse {
                s3_bind_addr: self.s3_addr.to_string(),
                admin_bind_addr: self.admin_addr.to_string(),
                admin_route_prefix: ADMIN_ROUTE_PREFIX.to_string(),
            },
            storage: StorageResponse {
                metadata_path: redact_path(&self.config.metadata_path().display().to_string()),
                data_dir: redact_path(&self.config.data_dir().display().to_string()),
                session_path: redact_path(
                    &self.transport_status.session_path.display().to_string(),
                ),
                buckets: metadata.buckets,
                committed_objects: metadata.committed_objects,
                active_objects: metadata.active_objects,
                staged_objects: metadata.staged_objects,
                recovery_markers: metadata.recovery_markers,
            },
            capacity: CapacityResponse {
                chunk_size: object.chunk_size,
                recovery_required_objects: object.recovery_required_objects,
                orphaned_chunks: object.orphaned_chunks,
            },
            telegram: TelegramResponse {
                session_state: format!("{:?}", self.transport_status.session_state),
                proxy_kind: format!("{:?}", self.transport_status.proxy_kind),
                proxy_url: self
                    .transport_status
                    .proxy_url
                    .as_deref()
                    .map(redact_proxy_url),
                phone_number: self
                    .config
                    .telegram_phone_number
                    .as_deref()
                    .map(redact_phone_number),
            },
            bootstrap: self.build_bootstrap_status(Some(session)),
        }
    }

    fn build_bootstrap_status(&self, session: Option<&SessionSnapshot>) -> BootstrapResponse {
        let checks = vec![
            check(
                "Telegram API ID",
                self.config
                    .telegram_api_id
                    .as_deref()
                    .is_some_and(|value| !value.is_empty()),
                "required for Telegram client bootstrap",
            ),
            check(
                "Telegram API hash",
                self.config
                    .telegram_api_hash
                    .as_deref()
                    .is_some_and(|value| !value.is_empty()),
                "required for Telegram client bootstrap",
            ),
            check(
                "Phone number",
                self.config
                    .telegram_phone_number
                    .as_deref()
                    .is_some_and(|value| !value.is_empty()),
                "needed for the Telegram login flow",
            ),
            check(
                "Session path",
                !self.transport_status.session_path.as_os_str().is_empty(),
                "Telegram session file is configured",
            ),
            check(
                "Telegram auth",
                matches!(
                    self.transport_status.session_state,
                    SessionState::Authorized | SessionState::Reused
                ),
                "Telegram session is usable",
            ),
            check(
                "Storage chat",
                self.config
                    .telegram_storage_chat_id
                    .as_deref()
                    .is_some_and(|value| !value.is_empty()),
                "dedicated private storage chat is configured",
            ),
            check(
                "Admin secret",
                !self.bootstrap_secret.is_empty(),
                "browser login gate is configured",
            ),
            check(
                "UI assets",
                self.ui_dist_dir.join("index.html").exists(),
                "Svelte build output is present",
            ),
        ];

        BootstrapResponse {
            ready: checks.iter().all(|check| check.ok),
            authenticated: session.is_some(),
            session_state: format!("{:?}", self.transport_status.session_state),
            phone_number: self
                .config
                .telegram_phone_number
                .as_deref()
                .map(redact_phone_number),
            proxy_mode: self.config.proxy_mode(),
            proxy_url: self
                .transport_status
                .proxy_url
                .as_deref()
                .map(redact_proxy_url),
            checks,
        }
    }
}

impl From<Option<&SessionSnapshot>> for SessionResponse {
    fn from(value: Option<&SessionSnapshot>) -> Self {
        match value {
            Some(session) => Self {
                authenticated: true,
                issued_at: Some(session.issued_at.clone()),
                expires_at: Some(session.expires_at.clone()),
                csrf_token: Some(session.csrf_token.clone()),
            },
            None => Self {
                authenticated: false,
                issued_at: None,
                expires_at: None,
                csrf_token: None,
            },
        }
    }
}

fn check(label: &str, ok: bool, detail: &str) -> CheckItem {
    CheckItem {
        label: label.to_string(),
        ok,
        detail: detail.to_string(),
    }
}

fn read_session_from_request(request: &Request<Incoming>, secret: &str) -> Option<SessionSnapshot> {
    let cookie_header = request.headers().get(header::COOKIE)?.to_str().ok()?;
    let value = parse_cookie(cookie_header, ADMIN_SESSION_COOKIE)?;
    decode_session(value, secret.as_bytes()).ok()
}

fn parse_cookie<'a>(cookie_header: &'a str, name: &str) -> Option<&'a str> {
    cookie_header.split(';').map(str::trim).find_map(|pair| {
        pair.split_once('=')
            .and_then(|(key, value)| (key == name).then_some(value))
    })
}

fn issue_session(secret: &[u8]) -> Result<IssuedSession, AdminError> {
    let issued_at = OffsetDateTime::now_utc();
    let expires_at = issued_at + Duration::seconds(ADMIN_SESSION_TTL_SECONDS);
    let claims = SessionClaimsWire {
        iat: issued_at.unix_timestamp(),
        exp: expires_at.unix_timestamp(),
        csrf: Uuid::new_v4().to_string(),
    };
    let payload = match serde_json::to_vec(&claims) {
        Ok(payload) => payload,
        Err(_) => {
            return Err(Box::new(json_error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "failed to create session",
            )));
        }
    };
    let key = hmac::Key::new(hmac::HMAC_SHA256, secret);
    let signature = hmac::sign(&key, &payload);
    let cookie_value = format!(
        "{}.{}",
        URL_SAFE_NO_PAD.encode(payload),
        URL_SAFE_NO_PAD.encode(signature.as_ref())
    );
    Ok(IssuedSession {
        snapshot: SessionSnapshot {
            authenticated: true,
            issued_at: rfc3339(issued_at),
            expires_at: rfc3339(expires_at),
            csrf_token: claims.csrf,
        },
        cookie_value,
    })
}

fn decode_session(value: &str, secret: &[u8]) -> Result<SessionSnapshot, AdminError> {
    let (payload, signature) = value.split_once('.').ok_or_else(|| {
        Box::new(json_error(
            StatusCode::UNAUTHORIZED,
            "invalid session cookie",
        ))
    })?;
    let payload_bytes = URL_SAFE_NO_PAD.decode(payload).map_err(|_| {
        Box::new(json_error(
            StatusCode::UNAUTHORIZED,
            "invalid session cookie",
        ))
    })?;
    let signature_bytes = URL_SAFE_NO_PAD.decode(signature).map_err(|_| {
        Box::new(json_error(
            StatusCode::UNAUTHORIZED,
            "invalid session cookie",
        ))
    })?;
    let key = hmac::Key::new(hmac::HMAC_SHA256, secret);
    hmac::verify(&key, &payload_bytes, &signature_bytes).map_err(|_| {
        Box::new(json_error(
            StatusCode::UNAUTHORIZED,
            "invalid session cookie",
        ))
    })?;
    let claims: SessionClaimsWire = serde_json::from_slice(&payload_bytes).map_err(|_| {
        Box::new(json_error(
            StatusCode::UNAUTHORIZED,
            "invalid session cookie",
        ))
    })?;
    let issued_at = OffsetDateTime::from_unix_timestamp(claims.iat).map_err(|_| {
        Box::new(json_error(
            StatusCode::UNAUTHORIZED,
            "invalid session cookie",
        ))
    })?;
    let expires_at = OffsetDateTime::from_unix_timestamp(claims.exp).map_err(|_| {
        Box::new(json_error(
            StatusCode::UNAUTHORIZED,
            "invalid session cookie",
        ))
    })?;
    if OffsetDateTime::now_utc() > expires_at {
        return Err(Box::new(json_error(
            StatusCode::UNAUTHORIZED,
            "session expired",
        )));
    }
    Ok(SessionSnapshot {
        authenticated: true,
        issued_at: rfc3339(issued_at),
        expires_at: rfc3339(expires_at),
        csrf_token: claims.csrf,
    })
}

fn require_csrf(request: &Request<Incoming>, session: &SessionSnapshot) -> bool {
    request
        .headers()
        .get(ADMIN_CSRF_HEADER)
        .and_then(|value| value.to_str().ok())
        .map(|value| constant_time_eq(value.as_bytes(), session.csrf_token.as_bytes()))
        .unwrap_or(false)
}

async fn read_json<T: for<'de> Deserialize<'de>>(
    request: Request<Incoming>,
) -> Result<T, AdminError> {
    let body = request
        .into_body()
        .collect()
        .await
        .map_err(|_| Box::new(json_error(StatusCode::BAD_REQUEST, "invalid request body")))?
        .to_bytes();
    serde_json::from_slice(&body)
        .map_err(|_| Box::new(json_error(StatusCode::BAD_REQUEST, "invalid request body")))
}

fn json_response<T: Serialize>(status: StatusCode, value: T) -> Response<Body> {
    let body = serde_json::to_vec(&value)
        .unwrap_or_else(|_| b"{\"error\":\"serialization failure\"}".to_vec());
    let mut response = Response::new(Body::from(Bytes::from(body)));
    *response.status_mut() = status;
    response.headers_mut().insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("application/json; charset=utf-8"),
    );
    response
        .headers_mut()
        .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    response
}

fn json_error(status: StatusCode, message: &str) -> Response<Body> {
    json_response(status, serde_json::json!({ "error": message }))
}

fn with_set_cookie(mut response: Response<Body>, cookie_value: String) -> Response<Body> {
    if let Ok(header) = HeaderValue::from_str(&cookie_header(cookie_value)) {
        response.headers_mut().append(header::SET_COOKIE, header);
    }
    response
}

fn with_clear_cookie(mut response: Response<Body>) -> Response<Body> {
    if let Ok(header) = HeaderValue::from_str(&expired_cookie()) {
        response.headers_mut().append(header::SET_COOKIE, header);
    }
    response
}

fn cookie_header(cookie_value: String) -> String {
    format!(
        "{name}={value}; Path={path}; HttpOnly; SameSite=Strict; Max-Age={ttl}",
        name = ADMIN_SESSION_COOKIE,
        value = cookie_value,
        path = ADMIN_COOKIE_PATH,
        ttl = ADMIN_SESSION_TTL_SECONDS,
    )
}

fn expired_cookie() -> String {
    format!(
        "{name}=; Path={path}; HttpOnly; SameSite=Strict; Max-Age=0",
        name = ADMIN_SESSION_COOKIE,
        path = ADMIN_COOKIE_PATH
    )
}

fn safe_join(base: &Path, relative: &str) -> Option<PathBuf> {
    let unsafe_path = Path::new(relative).components().any(|component| {
        matches!(
            component,
            Component::ParentDir | Component::RootDir | Component::Prefix(_)
        )
    });
    if unsafe_path {
        None
    } else {
        Some(base.join(relative))
    }
}

async fn read_static_file(path: &Path) -> Result<Option<Response<Body>>, std::io::Error> {
    let bytes = match fs::read(path).await {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error),
    };
    let mut response = Response::new(Body::from(Bytes::from(bytes)));
    response.headers_mut().insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static(content_type(path)),
    );
    response.headers_mut().insert(
        header::CACHE_CONTROL,
        HeaderValue::from_static(
            if path.file_name().and_then(|name| name.to_str()) == Some("index.html") {
                "no-store"
            } else {
                "public, max-age=31536000, immutable"
            },
        ),
    );
    Ok(Some(response))
}

async fn read_static_asset_fallback(
    base: &Path,
    requested_asset: &str,
) -> Result<Option<Response<Body>>, std::io::Error> {
    let requested_extension = Path::new(requested_asset)
        .extension()
        .and_then(|value| value.to_str())
        .unwrap_or_default();
    if requested_extension.is_empty() {
        return Ok(None);
    }

    let assets_dir = base.join("assets");
    let mut directory = match fs::read_dir(&assets_dir).await {
        Ok(directory) => directory,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error),
    };

    while let Some(entry) = directory.next_entry().await? {
        let path = entry.path();
        if path
            .extension()
            .and_then(|value| value.to_str())
            .is_some_and(|extension| extension == requested_extension)
        {
            return read_static_file(&path).await;
        }
    }

    Ok(None)
}

fn content_type(path: &Path) -> &'static str {
    match path
        .extension()
        .and_then(|value| value.to_str())
        .unwrap_or_default()
    {
        "html" => "text/html; charset=utf-8",
        "css" => "text/css; charset=utf-8",
        "js" | "mjs" => "application/javascript; charset=utf-8",
        "json" => "application/json; charset=utf-8",
        "svg" => "image/svg+xml",
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "webp" => "image/webp",
        "ico" => "image/x-icon",
        "map" => "application/json; charset=utf-8",
        _ => "application/octet-stream",
    }
}

fn empty_metadata_status() -> MetadataStatus {
    MetadataStatus {
        path: None,
        schema_version: 0,
        buckets: 0,
        committed_objects: 0,
        active_objects: 0,
        staged_objects: 0,
        recovery_markers: 0,
    }
}

fn empty_object_status() -> ObjectFormatStatus {
    ObjectFormatStatus {
        data_dir: PathBuf::new(),
        chunk_size: 0,
        committed_objects: 0,
        staged_objects: 0,
        recovery_required_objects: 0,
        orphaned_chunks: 0,
    }
}

fn redact_proxy_url(url: &str) -> String {
    match url::Url::parse(url) {
        Ok(parsed) => {
            let host = parsed.host_str().unwrap_or("<unknown>");
            let port = parsed
                .port()
                .map_or(String::new(), |port| format!(":{port}"));
            format!("{}://{}{}", parsed.scheme(), host, port)
        }
        Err(_) => "<redacted-proxy>".to_string(),
    }
}

fn constant_time_eq(left: &[u8], right: &[u8]) -> bool {
    if left.len() != right.len() {
        return false;
    }
    let mut diff = 0u8;
    for (left_byte, right_byte) in left.iter().zip(right) {
        diff |= left_byte ^ right_byte;
    }
    diff == 0
}

fn rfc3339(value: OffsetDateTime) -> String {
    value
        .format(&time::format_description::well_known::Rfc3339)
        .unwrap_or_else(|_| value.unix_timestamp().to_string())
}
