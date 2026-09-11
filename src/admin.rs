//! Authenticated operator/admin HTTP surface for the `/_admin` SPA.
//!
//! Serves the (self-hosted, hidden-behind-login) management console on the same
//! public listener as the S3 data plane. Authentication is credential based:
//! accounts live in `metadata::users` (argon2id), sessions are signed cookies
//! bound to a `admin_sessions` row so they can be individually revoked and are
//! invalidated globally on password change / user disable via `token_version`.
//!
//! Phase-9 core scope: login / logout / refresh / whoami, user CRUD, and the
//! JSON file-management surface (bucket create/list/delete-empty +
//! prefix/folder listing + zero-byte directory markers + tombstones). Binary
//! content streaming (upload/download) and the in-browser Telegram onboarding
//! wizard are now wired here as the landed follow-up increment (see ADR-0006 /
//! ROADMAP).

mod phase10;
use crate::auth::{self, AuthError, LoginLimiter};
use crate::config::{
    AppConfig, normalize_telegram_storage_chat_id, validate_telegram_bootstrap_settings,
};
use crate::manifest::ObjectManifest;
use crate::metadata::{
    MetadataStore, RecoveryAck, RecoveryAcknowledgements, TelegramBootstrapSettings,
};
use crate::object_format::{ObjectFormatService, RecoveryIssue as RecoveryIssueModel};
use crate::redact::redact_path;
use crate::telegram::{
    LoginDriverError, LoginStage, SessionState, TelegramConnectionHealth, TelegramLoginDriver,
    TelegramTransportManager,
};
use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use bytes::Bytes;
use futures::{FutureExt, StreamExt};
use http::header::{self, HeaderValue};
use http::{Method, StatusCode};
use http_body_util::BodyExt;
use http_body_util::StreamBody;
use hyper::body::{Frame, Incoming};
use hyper::{Request, Response};
use ring::hmac;
use s3s::Body;
use s3s::dto::StreamingBlob;
use serde::{Deserialize, Serialize};
use std::panic::AssertUnwindSafe;
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
const ADMIN_SESSION_TTL_SECONDS: i64 = 8 * 60 * 60;
const ADMIN_CSRF_HEADER: &str = "x-csrf-token";

#[derive(Clone)]
pub struct AdminUiState {
    config: AppConfig,
    object_format: Arc<ObjectFormatService>,
    transport_manager: Arc<TelegramTransportManager>,
    /// Process-wide, single in-flight onboarding flow state.
    wizard_driver: Arc<tokio::sync::Mutex<TelegramLoginDriver>>,
    cookie_secret: String,
    ui_dist_dir: PathBuf,
    limiter: LoginLimiter,
}

// ---- wire types -------------------------------------------------------------

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "snake_case")]
struct UserWire {
    id: String,
    username: String,
    display_name: String,
    role: String,
    disabled: bool,
}

impl UserWire {
    fn from_user(user: &crate::metadata::DbUser) -> Self {
        Self {
            id: user.id.clone(),
            username: user.username.clone(),
            display_name: user.display_name.clone(),
            role: user.role.clone(),
            disabled: user.disabled,
        }
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "snake_case")]
struct SessionResponse {
    authenticated: bool,
    user: Option<UserWire>,
    issued_at: Option<String>,
    expires_at: Option<String>,
    csrf_token: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "snake_case")]
struct LoginRequest {
    username: String,
    password: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "snake_case")]
struct CreateUserRequest {
    username: String,
    password: String,
    #[serde(default)]
    display_name: String,
    #[serde(default)]
    role: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "snake_case")]
struct ChangePasswordRequest {
    password: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "snake_case")]
struct CreateFolderRequest {
    bucket: String,
    path: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "snake_case")]
struct CreateBucketRequest {
    name: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "snake_case")]
struct DeleteObjectRequest {
    bucket: String,
    key: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "snake_case")]
struct RecoveryAcknowledgeRequest {
    /// Recovery issue fingerprints, as served in `recovery.issues[].id`.
    ids: Vec<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "snake_case")]
struct TelegramSettingsRequest {
    telegram_api_id: Option<String>,
    telegram_api_hash: Option<String>,
    telegram_storage_chat_id: Option<String>,
    telegram_proxy_url: Option<String>,
    telegram_proxy_username: Option<String>,
    telegram_proxy_password: Option<String>,
    telegram_proxy_mode: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "snake_case")]
struct TelegramDisconnectRequest {
    #[serde(default)]
    delete_uploaded_files: bool,
    #[serde(default)]
    phone_confirmation: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "snake_case")]
struct BeginResumableRequest {
    bucket: String,
    key: String,
    #[serde(default = "default_content_type")]
    content_type: String,
    #[serde(default)]
    expires_in_seconds: Option<u64>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "snake_case")]
struct CreateShareRequest {
    bucket: String,
    key: String,
    #[serde(default)]
    expires_in_seconds: Option<u64>,
    #[serde(default)]
    description: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "snake_case")]
struct UpdateShareRequest {
    #[serde(default)]
    expires_in_seconds: Option<u64>,
}

fn default_content_type() -> String {
    "application/octet-stream".to_string()
}

// Session claims carried in the signed cookie and mirrored into the DB row.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct SessionClaims {
    uid: String,
    sid: String,
    ver: i64,
    iat: i64,
    exp: i64,
    csrf: String,
}

struct ResolvedPrincipal {
    user: crate::metadata::DbUser,
    claims: SessionClaims,
}

struct IssuedSession {
    user: UserWire,
    cookie_value: String,
    issued_at: String,
    expires_at: String,
    csrf_token: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "snake_case")]
struct ObjectEntryWire {
    name: String,
    key: String,
    size: u64,
    last_modified: String,
    etag: String,
    expires_at: Option<String>,
    shared_links: u64,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "snake_case")]
struct ListObjectsResponse {
    prefix: String,
    folders: Vec<String>,
    objects: Vec<ObjectEntryWire>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "snake_case")]
struct ShareLinkWire {
    id: String,
    url: Option<String>,
    description: String,
    created_at: String,
    expires_at: Option<String>,
    status: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "snake_case")]
struct ShareLinksResponse {
    bucket: String,
    key: String,
    links: Vec<ShareLinkWire>,
    count: usize,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "snake_case")]
struct BucketEntryWire {
    name: String,
    created_at: String,
}

fn rfc3339_unix(unix: i64) -> String {
    OffsetDateTime::from_unix_timestamp(unix)
        .map(|value| {
            value
                .format(&time::format_description::well_known::Rfc3339)
                .unwrap_or_else(|_| unix.to_string())
        })
        .unwrap_or_else(|_| unix.to_string())
}

fn rfc3339_unix_opt(unix: i64) -> Option<String> {
    OffsetDateTime::from_unix_timestamp(unix)
        .ok()
        .and_then(|value| {
            value
                .format(&time::format_description::well_known::Rfc3339)
                .ok()
        })
}

fn expiry_from_seconds(value: Option<u64>) -> Result<Option<OffsetDateTime>, String> {
    let Some(seconds) = value else {
        return Ok(None);
    };
    if seconds == 0 || seconds > i64::MAX as u64 {
        return Err("expiry seconds must be between 1 and 9223372036854775807".to_string());
    }
    Ok(Some(
        OffsetDateTime::now_utc() + time::Duration::seconds(seconds as i64),
    ))
}

fn rfc3339(value: OffsetDateTime) -> String {
    value
        .format(&time::format_description::well_known::Rfc3339)
        .unwrap_or_else(|_| value.unix_timestamp().to_string())
}

impl AdminUiState {
    pub fn new(
        config: AppConfig,
        object_format: Arc<ObjectFormatService>,
        transport_manager: Arc<TelegramTransportManager>,
        cookie_secret: String,
        ui_dist_dir: PathBuf,
    ) -> Self {
        Self {
            config,
            object_format,
            transport_manager,
            wizard_driver: Arc::new(tokio::sync::Mutex::new(TelegramLoginDriver::new())),
            cookie_secret,
            ui_dist_dir,
            limiter: LoginLimiter::new(),
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
            return self.handle_api(request).await;
        }
        self.handle_static(request).await
    }

    async fn telegram_health_snapshot(&self) -> TelegramConnectionHealth {
        self.transport_manager.health().await
    }

    fn store(&self) -> &MetadataStore {
        self.object_format.metadata_store()
    }

    // ---- routing -------------------------------------------------------------

    async fn handle_api(self: Arc<Self>, request: Request<Incoming>) -> Response<Body> {
        let method = request.method().clone();
        let path = request.uri().path().to_string();
        // Trim admin/api prefix for a friendlier match on the remaining path.
        let rest = path
            .strip_prefix(ADMIN_API_PREFIX)
            .unwrap_or(&path[..ADMIN_API_PREFIX.len().min(path.len())]);

        if method == Method::GET && rest == "setup" {
            return match self.store().setup_required() {
                Ok(required) => json_response(
                    StatusCode::OK,
                    serde_json::json!({"setup_required":required}),
                ),
                Err(_) => json_error(StatusCode::INTERNAL_SERVER_ERROR, "setup state unavailable"),
            };
        }
        if method == Method::POST && rest == "setup" {
            return self.setup_account(request).await;
        }
        if method == Method::POST && !phase10::same_origin(&request) {
            return json_error(StatusCode::FORBIDDEN, "cross-origin request rejected");
        }
        if method == Method::GET && rest == "session" {
            return match self.principal_from_headers(request.headers()) {
                Ok(Some(principal)) => json_response(
                    StatusCode::OK,
                    SessionResponse {
                        authenticated: true,
                        user: Some(UserWire::from_user(&principal.user)),
                        issued_at: Some(rfc3339_unix(principal.claims.iat)),
                        expires_at: Some(rfc3339_unix(principal.claims.exp)),
                        csrf_token: Some(principal.claims.csrf),
                    },
                ),
                Ok(None) => session_anonymous(),
                Err(error) => auth_error_response(&error),
            };
        }

        if method == Method::POST && rest == "session/login" {
            return match self.handle_login(request).await {
                Ok(response) => response,
                Err(error) => auth_error_response(&error),
            };
        }

        // Every remaining route needs an authenticated principal.
        let principal = match self.principal_from_headers(request.headers()) {
            Ok(Some(principal)) => principal,
            Ok(None) => return json_error(StatusCode::UNAUTHORIZED, "not authenticated"),
            Err(error) => return auth_error_response(&error),
        };

        let is_mutating = matches!(
            method,
            Method::POST | Method::PUT | Method::DELETE | Method::PATCH
        );
        if is_mutating && !require_csrf(&request, &principal.claims) {
            return json_error(StatusCode::FORBIDDEN, "invalid csrf token");
        }

        if rest == "jobs" || rest.starts_with("jobs/") {
            return self.job_api(request, rest);
        }
        if method == Method::POST && rest == "uploads" {
            return self.enqueue_upload(request).await;
        }
        if method == Method::POST && rest == "uploads/resumable" {
            return self.begin_resumable_upload(request).await;
        }
        if rest.starts_with("uploads/resumable/") {
            return self.resumable_upload_api(request, rest).await;
        }
        if method == Method::POST && rest == "recovery/repair" {
            self.object_format.ensure_workers();
            return self.handle_recovery_repair().await;
        }
        if method == Method::POST && rest == "recovery/acknowledge" {
            return self
                .handle_recovery_acknowledge(request, &principal, true)
                .await;
        }
        if method == Method::POST && rest == "recovery/unacknowledge" {
            return self
                .handle_recovery_acknowledge(request, &principal, false)
                .await;
        }
        match (method, rest) {
            (Method::POST, "session/logout") => self.handle_logout(&principal).await,
            (Method::POST, "session/refresh") => self.handle_refresh(&principal).await,
            (Method::GET, "overview") => self.handle_overview(&principal).await,
            (Method::GET, "users") => self.handle_list_users(),
            (Method::POST, "users") => self.handle_create_user(request, &principal).await,
            (Method::POST, p) if p.starts_with("users/") => {
                self.handle_change_password(request, p, &principal).await
            }
            (Method::DELETE, p) if p.starts_with("users/") => {
                self.handle_delete_user(p, &principal)
            }
            (Method::GET, "buckets") => self.handle_list_buckets(),
            (Method::POST, "buckets") => self.handle_create_bucket(request).await,
            (Method::DELETE, p) if p.starts_with("buckets/") => self.handle_delete_bucket(p),
            (Method::GET, "objects") => self.handle_list_objects(request),
            (Method::POST, "objects/folder") => {
                self.handle_create_folder(request, &principal).await
            }
            (Method::POST, "objects/delete") => {
                self.handle_delete_object(request, &principal).await
            }
            (Method::POST, "objects/share") => self.handle_create_share(request).await,
            (Method::GET, "objects/shares") => self.handle_list_share_links(request),
            (Method::PATCH, p) if p.starts_with("objects/shares/") => {
                self.handle_update_share_link(request, p).await
            }
            (Method::DELETE, p) if p.starts_with("objects/shares/") => {
                self.handle_revoke_share_link(p)
            }
            (Method::POST, "objects/content") => {
                self.handle_upload_content(request, &principal).await
            }
            (Method::GET, "objects/content") => self.handle_content(request, &principal, false),
            (Method::HEAD, "objects/content") => self.handle_content(request, &principal, true),
            (Method::GET, "telegram/wizard/state") => self.wizard_state(&principal).await,
            (Method::POST, "telegram/wizard/begin") => self.wizard_begin(request, &principal).await,
            (Method::POST, "telegram/wizard/submit-code") => {
                self.wizard_submit_code(request, &principal).await
            }
            (Method::POST, "telegram/wizard/submit-password") => {
                self.wizard_submit_password(request, &principal).await
            }
            (Method::POST, "telegram/wizard/cancel") => {
                self.wizard_cancel(request, &principal).await
            }
            (Method::GET, "telegram/settings") => self.telegram_settings(&principal).await,
            (Method::POST, "telegram/settings") => {
                self.telegram_save_settings(request, &principal).await
            }
            (Method::GET, "telegram/storage-settings") => self.telegram_storage_settings().await,
            (Method::POST, "telegram/storage-settings") => {
                self.telegram_save_storage_settings(request).await
            }
            (Method::POST, "telegram/disconnect") => self.telegram_disconnect(request).await,
            _ => json_error(StatusCode::NOT_FOUND, "not found"),
        }
    }

    // ---- auth actions --------------------------------------------------------

    fn principal_from_headers(
        &self,
        headers: &http::HeaderMap,
    ) -> Result<Option<ResolvedPrincipal>, AuthError> {
        let Some(cookie_value) = read_session_cookie(headers) else {
            return Ok(None);
        };
        let claims = match decode_claims(&cookie_value, self.cookie_secret.as_bytes()) {
            Ok(claims) => claims,
            Err(_) => return Ok(None),
        };
        if OffsetDateTime::now_utc().unix_timestamp() > claims.exp {
            return Ok(None);
        }
        // Validate against the authoritative session row + account state.
        let session = self
            .store()
            .get_session(&claims.sid)
            .map_err(|error| AuthError {
                kind: crate::auth::AuthErrorKind::Internal,
                message: format!("session store error: {error}"),
                retry_after_secs: None,
            })?;
        let Some(session) = session else {
            return Ok(None);
        };
        if session.revoked_at.is_some() {
            return Ok(None);
        }
        let Some(user) = self
            .store()
            .get_user_by_id(&claims.uid)
            .map_err(|error| AuthError {
                kind: crate::auth::AuthErrorKind::Internal,
                message: format!("user store error: {error}"),
                retry_after_secs: None,
            })?
        else {
            return Ok(None);
        };
        if user.disabled || user.token_version != claims.ver {
            return Ok(None);
        }
        Ok(Some(ResolvedPrincipal { user, claims }))
    }

    async fn handle_login(&self, request: Request<Incoming>) -> Result<Response<Body>, AuthError> {
        let LoginRequest { username, password } = match read_json::<LoginRequest>(request).await {
            Ok(login) => login,
            Err(_) => {
                let error = AuthError {
                    kind: auth::AuthErrorKind::InvalidUsername,
                    message: "invalid login payload".to_string(),
                    retry_after_secs: None,
                };
                return Err(error);
            }
        };
        let keys = LoginLimiter::keys_for(None, &username);
        if let Some(error) = self.limiter.check(&keys) {
            self.limiter.record_failure(&keys);
            return Err(error);
        }

        // Verify is CPU-heavy; run off the async reactor via a blocking thread.
        let object = Arc::clone(&self.object_format);
        let username_c = username.clone();
        let password_c = password.clone();
        let verified = tokio::task::spawn_blocking(move || {
            let store = object.metadata_store();
            auth::authenticate(store, &username_c, &password_c)
        })
        .await
        .map_err(|error| AuthError {
            kind: auth::AuthErrorKind::Internal,
            message: format!("auth worker error: {error}"),
            retry_after_secs: None,
        })?;

        let user = match verified {
            Ok(user) => user,
            Err(error) => {
                self.limiter.record_failure(&keys);
                return Err(mask_auth_error(error));
            }
        };
        self.limiter.reset(&keys);

        let issued = self.issue_session(&user, None)?;
        let mut response = json_response(
            StatusCode::OK,
            SessionResponse {
                authenticated: true,
                user: Some(issued.user),
                issued_at: Some(issued.issued_at),
                expires_at: Some(issued.expires_at),
                csrf_token: Some(issued.csrf_token),
            },
        );
        with_set_cookie(&mut response, issued.cookie_value);
        Ok(response)
    }

    async fn handle_logout(&self, principal: &ResolvedPrincipal) -> Response<Body> {
        let _ = self.store().revoke_session(&principal.claims.sid);
        let mut response = json_response(
            StatusCode::OK,
            SessionResponse {
                authenticated: false,
                user: None,
                issued_at: None,
                expires_at: None,
                csrf_token: None,
            },
        );
        with_clear_cookie(&mut response);
        response
    }

    async fn handle_refresh(&self, principal: &ResolvedPrincipal) -> Response<Body> {
        let issued = match self.issue_session(&principal.user, None) {
            Ok(issued) => issued,
            Err(error) => return auth_error_response(&error),
        };
        let mut response = json_response(
            StatusCode::OK,
            SessionResponse {
                authenticated: true,
                user: Some(issued.user),
                issued_at: Some(issued.issued_at),
                expires_at: Some(issued.expires_at),
                csrf_token: Some(issued.csrf_token),
            },
        );
        with_set_cookie(&mut response, issued.cookie_value);
        response
    }

    fn issue_session(
        &self,
        user: &crate::metadata::DbUser,
        _ip: Option<std::net::IpAddr>,
    ) -> Result<IssuedSession, AuthError> {
        let now = OffsetDateTime::now_utc();
        let started = now.unix_timestamp();
        let expires = (now + Duration::seconds(ADMIN_SESSION_TTL_SECONDS)).unix_timestamp();
        let csrf = Uuid::new_v4().to_string();
        let sid = Uuid::new_v4().to_string();
        let claims = SessionClaims {
            uid: user.id.clone(),
            sid: sid.clone(),
            ver: user.token_version,
            iat: started,
            exp: expires,
            csrf: csrf.clone(),
        };
        let payload = serde_json::to_vec(&claims)
            .map_err(|error| internal_auth(format!("session serialize failed: {error}")))?;
        let cookie_value = sign_payload(&payload, self.cookie_secret.as_bytes());
        // Persist authoritative session for revocation tracking.
        self.store()
            .insert_session(&crate::metadata::DbSession {
                cookie_id: sid,
                user_id: user.id.clone(),
                token_version: user.token_version,
                issued_at: OffsetDateTime::from_unix_timestamp(started).unwrap_or(now),
                expires_at: OffsetDateTime::from_unix_timestamp(expires).unwrap_or_else(|_| {
                    OffsetDateTime::now_utc() + Duration::seconds(ADMIN_SESSION_TTL_SECONDS)
                }),
                created_ip: None,
                revoked_at: None,
            })
            .map_err(|error| internal_auth(format!("session persist failed: {error}")))?;
        Ok(IssuedSession {
            user: UserWire::from_user(user),
            cookie_value,
            issued_at: rfc3339_unix(started),
            expires_at: rfc3339_unix(expires),
            csrf_token: csrf,
        })
    }

    // ---- user management -----------------------------------------------------

    fn handle_list_users(&self) -> Response<Body> {
        let users = match self.store().list_users() {
            Ok(users) => users,
            Err(error) => return json_error(StatusCode::INTERNAL_SERVER_ERROR, &error.to_string()),
        };
        let payload = users.iter().map(UserWire::from_user).collect::<Vec<_>>();
        json_response(StatusCode::OK, serde_json::json!({ "users": payload }))
    }

    async fn handle_create_user(
        &self,
        request: Request<Incoming>,
        principal: &ResolvedPrincipal,
    ) -> Response<Body> {
        if !auth::is_superadmin(&principal.user) {
            return json_error(StatusCode::FORBIDDEN, "superadmin privilege required");
        }
        let CreateUserRequest {
            username,
            password,
            display_name,
            role,
        } = match read_json::<CreateUserRequest>(request).await {
            Ok(body) => body,
            Err(_) => return json_error(StatusCode::BAD_REQUEST, "invalid user payload"),
        };
        let role = if role.is_empty() {
            auth::ROLE_ADMIN.to_string()
        } else if role != auth::ROLE_ADMIN && role != auth::ROLE_SUPERADMIN {
            return json_error(StatusCode::BAD_REQUEST, "invalid role");
        } else {
            role
        };
        let user =
            match auth::create_account(self.store(), &username, &password, &role, &display_name) {
                Ok(user) => user,
                Err(error) => return auth_error_response(&error),
            };
        json_response(StatusCode::CREATED, UserWire::from_user(&user))
    }

    async fn handle_change_password(
        &self,
        request: Request<Incoming>,
        id_path: &str,
        principal: &ResolvedPrincipal,
    ) -> Response<Body> {
        let id = id_path.trim_start_matches("users/");
        let Ok(target) = self.store().get_user_by_id(id) else {
            return json_error(StatusCode::NOT_FOUND, "user not found");
        };
        let Some(target) = target else {
            return json_error(StatusCode::NOT_FOUND, "user not found");
        };
        let is_self = target.id == principal.user.id;
        if !is_self && !auth::is_superadmin(&principal.user) {
            return json_error(
                StatusCode::FORBIDDEN,
                "not allowed to change this user's password",
            );
        }
        let ChangePasswordRequest { password } =
            match read_json::<ChangePasswordRequest>(request).await {
                Ok(body) => body,
                Err(_) => return json_error(StatusCode::BAD_REQUEST, "invalid payload"),
            };
        if let Err(error) = auth::change_password(self.store(), &target.id, &password) {
            return auth_error_response(&error);
        }
        if is_self {
            // Our own token_version changed; reissue for continuity with the new
            // session state and return the fresh cookie.
            return match self
                .store()
                .get_user_by_id(&target.id)
                .ok()
                .flatten()
                .map(|user| self.issue_session(&user, None))
            {
                Some(Ok(issued)) => {
                    let mut response = json_response(
                        StatusCode::OK,
                        SessionResponse {
                            authenticated: true,
                            user: Some(issued.user),
                            issued_at: Some(issued.issued_at),
                            expires_at: Some(issued.expires_at),
                            csrf_token: Some(issued.csrf_token),
                        },
                    );
                    with_set_cookie(&mut response, issued.cookie_value);
                    response
                }
                _ => json_response(StatusCode::OK, serde_json::json!({ "ok": true })),
            };
        }
        json_response(StatusCode::OK, serde_json::json!({ "ok": true }))
    }

    fn handle_delete_user(&self, id_path: &str, principal: &ResolvedPrincipal) -> Response<Body> {
        if !auth::is_superadmin(&principal.user) {
            return json_error(StatusCode::FORBIDDEN, "superadmin privilege required");
        }
        let id = id_path.trim_start_matches("users/");
        let Ok(Some(target)) = self.store().get_user_by_id(id) else {
            return json_error(StatusCode::NOT_FOUND, "user not found");
        };
        if auth::is_superadmin(&target) && self.store().enabled_superadmin_count().unwrap_or(0) <= 1
        {
            return json_error(StatusCode::CONFLICT, "cannot delete the last superadmin");
        }
        if let Err(error) = self.store().delete_user(id) {
            return json_error(StatusCode::INTERNAL_SERVER_ERROR, &error.to_string());
        }
        let _ = self.store().revoke_user_sessions(&target.id);
        json_response(StatusCode::OK, serde_json::json!({ "ok": true }))
    }

    // ---- file-management (JSON) ----------------------------------------------

    fn handle_list_buckets(&self) -> Response<Body> {
        let buckets = match self.object_format.list_buckets() {
            Ok(buckets) => buckets,
            Err(error) => return json_error(StatusCode::INTERNAL_SERVER_ERROR, &error.to_string()),
        };
        let wire = buckets
            .into_iter()
            .map(|bucket| BucketEntryWire {
                name: bucket.name,
                created_at: rfc3339(bucket.created_at),
            })
            .collect::<Vec<_>>();
        json_response(StatusCode::OK, serde_json::json!({ "buckets": wire }))
    }

    async fn handle_create_bucket(&self, request: Request<Incoming>) -> Response<Body> {
        let CreateBucketRequest { name } = match read_json::<CreateBucketRequest>(request).await {
            Ok(body) => body,
            Err(_) => return json_error(StatusCode::BAD_REQUEST, "invalid bucket payload"),
        };
        let bucket = name.trim();
        if bucket.is_empty() {
            return json_error(StatusCode::BAD_REQUEST, "bucket name is required");
        }
        match self.object_format.create_bucket(bucket) {
            Ok(created) => json_response(
                StatusCode::CREATED,
                BucketEntryWire {
                    name: created.name,
                    created_at: rfc3339(created.created_at),
                },
            ),
            Err(error) => bucket_error_response(&error),
        }
    }

    fn handle_delete_bucket(&self, id_path: &str) -> Response<Body> {
        let encoded_bucket = id_path.trim_start_matches("buckets/");
        let bucket = match urlencoding::decode(encoded_bucket) {
            Ok(value) => value.into_owned(),
            Err(_) => return json_error(StatusCode::BAD_REQUEST, "invalid bucket name"),
        };
        if bucket.is_empty() {
            return json_error(StatusCode::BAD_REQUEST, "bucket name is required");
        }
        match self.object_format.delete_bucket(&bucket) {
            Ok(()) => json_response(StatusCode::OK, serde_json::json!({ "ok": true })),
            Err(error) => bucket_error_response(&error),
        }
    }

    fn handle_list_objects(&self, request: Request<Incoming>) -> Response<Body> {
        let query = parse_list_params(request.uri().query().unwrap_or(""));
        let bucket = match query.get("bucket") {
            Some(value) if !value.is_empty() => value.clone(),
            _ => return json_error(StatusCode::BAD_REQUEST, "bucket is required"),
        };
        let prefix = query.get("prefix").cloned().unwrap_or_default();
        if !prefix.is_empty() && !prefix.ends_with('/') {
            return json_error(StatusCode::BAD_REQUEST, "prefix must end with '/'");
        }
        if prefix.len() > 2048 {
            return json_error(StatusCode::BAD_REQUEST, "prefix too long");
        }
        let manifests = match self
            .object_format
            .list_bucket_manifests(&bucket, Some(&prefix))
        {
            Ok(items) => items,
            Err(error) => return json_error(StatusCode::INTERNAL_SERVER_ERROR, &error.to_string()),
        };

        let delimiter = query.contains_key("delimiter");
        let mut folders: Vec<String> = Vec::new();
        let mut objects: Vec<ObjectEntryWire> = Vec::new();

        for manifest in manifests {
            let key = &manifest.key;
            let relative = key.strip_prefix(&prefix).unwrap_or(key);
            if delimiter && let Some(slash) = relative.find('/') {
                let folder = prefix.clone() + &relative[..=slash];
                if !folders.contains(&folder) {
                    folders.push(folder);
                }
                continue;
            }
            if relative.is_empty() {
                continue;
            }
            let shared_links = match self.store().share_link_count(&bucket, key) {
                Ok(count) => count,
                Err(error) => {
                    return json_error(StatusCode::INTERNAL_SERVER_ERROR, &error.to_string());
                }
            };
            objects.push(object_to_wire(&manifest, key, shared_links));
        }
        folders.sort();
        objects.sort_by(|a, b| a.name.cmp(&b.name));
        // Report only basenames for folders/objects at this level.
        let folder_names = folders
            .iter()
            .map(|path| {
                path.strip_prefix(&prefix)
                    .unwrap_or(path)
                    .trim_end_matches('/')
                    .to_string()
            })
            .collect::<Vec<_>>();
        json_response(
            StatusCode::OK,
            ListObjectsResponse {
                prefix: prefix.clone(),
                folders: folder_names,
                objects,
            },
        )
    }

    async fn handle_create_folder(
        &self,
        request: Request<Incoming>,
        _principal: &ResolvedPrincipal,
    ) -> Response<Body> {
        let CreateFolderRequest { bucket, path } =
            match read_json::<CreateFolderRequest>(request).await {
                Ok(body) => body,
                Err(_) => return json_error(StatusCode::BAD_REQUEST, "invalid payload"),
            };
        if path.is_empty() || !path.ends_with('/') {
            return json_error(StatusCode::BAD_REQUEST, "path must end with '/'");
        }
        if !is_safe_folder_path(&path) {
            return json_error(StatusCode::BAD_REQUEST, "invalid path");
        }
        if !matches!(self.object_format.bucket_exists(&bucket), Ok(true)) {
            return json_error(StatusCode::NOT_FOUND, "bucket not found");
        }
        // Write an empty directory-marker object (S3-visible zero-byte key). The
        // S3 store has no native directory primitive; this keeps it visible to
        // other S3 clients and is used to persist truly-empty folders.
        let manifest = match self
            .object_format
            .put_bytes(&bucket, &path, "application/directory", &[])
            .await
        {
            Ok(manifest) => manifest,
            Err(error) => {
                return json_error(StatusCode::INTERNAL_SERVER_ERROR, &error.to_string());
            }
        };
        let _ = manifest;
        json_response(StatusCode::CREATED, serde_json::json!({ "ok": true }))
    }

    async fn handle_delete_object(
        &self,
        request: Request<Incoming>,
        _principal: &ResolvedPrincipal,
    ) -> Response<Body> {
        let DeleteObjectRequest { bucket, key } =
            match read_json::<DeleteObjectRequest>(request).await {
                Ok(body) => body,
                Err(_) => return json_error(StatusCode::BAD_REQUEST, "invalid payload"),
            };
        if key.is_empty() {
            return json_error(StatusCode::BAD_REQUEST, "key is required");
        }
        let deletion = if key.ends_with('/') {
            self.object_format.delete_empty_folder(&bucket, &key)
        } else {
            self.object_format
                .delete_object(&bucket, &key, None, None, None)
        };
        match deletion {
            Ok(Some(_)) => json_response(
                StatusCode::OK,
                serde_json::json!({ "ok": true, "deleted": true }),
            ),
            Ok(None) => json_error(StatusCode::NOT_FOUND, "object not found"),
            Err(crate::object_format::ObjectFormatError::Metadata(
                crate::metadata::MetadataError::FolderNotEmpty(folder),
            )) => json_error(StatusCode::CONFLICT, &format!("folder not empty: {folder}")),
            Err(error) => json_error(StatusCode::INTERNAL_SERVER_ERROR, &error.to_string()),
        }
    }

    async fn handle_create_share(&self, request: Request<Incoming>) -> Response<Body> {
        let body = match read_json::<CreateShareRequest>(request).await {
            Ok(body) => body,
            Err(_) => return json_error(StatusCode::BAD_REQUEST, "invalid share payload"),
        };
        if body.bucket.is_empty() || !is_safe_object_key(&body.key) {
            return json_error(
                StatusCode::BAD_REQUEST,
                "bucket and valid object key required",
            );
        }
        let manifest = match self
            .object_format
            .get_active_manifest(&body.bucket, &body.key)
        {
            Ok(Some(manifest)) => manifest,
            Ok(None) => return json_error(StatusCode::NOT_FOUND, "object not found"),
            Err(error) => return json_error(StatusCode::INTERNAL_SERVER_ERROR, &error.to_string()),
        };
        let requested_expiry = match expiry_from_seconds(body.expires_in_seconds) {
            Ok(value) => value,
            Err(message) => return json_error(StatusCode::BAD_REQUEST, &message),
        };
        let object_expiry = manifest.expires_at;
        let expires_at = match (requested_expiry, object_expiry) {
            (Some(requested), Some(object)) => Some(requested.min(object)),
            (Some(requested), None) => Some(requested),
            (None, object) => object,
        };
        if expires_at.is_some_and(|value| value <= OffsetDateTime::now_utc()) {
            return json_error(
                StatusCode::BAD_REQUEST,
                "share expiry must be before object expiry",
            );
        }
        let expires_at = expires_at.map(|value| value.unix_timestamp());
        let (token, record) =
            match self
                .object_format
                .create_share_link(&manifest, expires_at, &body.description)
            {
                Ok(value) => value,
                Err(error) => {
                    return json_error(StatusCode::INTERNAL_SERVER_ERROR, &error.to_string());
                }
            };
        json_response(
            StatusCode::CREATED,
            serde_json::json!({
                "id": record.id,
                "url": format!("/_public/{token}"),
                "object": {"bucket": record.bucket, "key": record.key},
                "description": record.description,
                "expires_at": record.expires_at.and_then(rfc3339_unix_opt),
            }),
        )
    }

    fn handle_list_share_links(&self, request: Request<Incoming>) -> Response<Body> {
        let query = parse_list_params(request.uri().query().unwrap_or(""));
        let bucket = match query.get("bucket") {
            Some(value) if !value.is_empty() => value.clone(),
            _ => return json_error(StatusCode::BAD_REQUEST, "bucket is required"),
        };
        let key = match query.get("key") {
            Some(value) if is_safe_object_key(value) => value.clone(),
            _ => return json_error(StatusCode::BAD_REQUEST, "valid object key is required"),
        };
        match self.object_format.get_active_manifest(&bucket, &key) {
            Ok(Some(_)) => {}
            Ok(None) => return json_error(StatusCode::NOT_FOUND, "object not found"),
            Err(error) => return json_error(StatusCode::INTERNAL_SERVER_ERROR, &error.to_string()),
        }
        let records = match self.object_format.list_share_links(&bucket, &key) {
            Ok(records) => records,
            Err(error) => return json_error(StatusCode::INTERNAL_SERVER_ERROR, &error.to_string()),
        };
        let links = records
            .iter()
            .map(|record| self.share_link_to_wire(record))
            .collect::<Vec<_>>();
        json_response(
            StatusCode::OK,
            ShareLinksResponse {
                bucket,
                key,
                count: links.len(),
                links,
            },
        )
    }

    async fn handle_update_share_link(
        &self,
        request: Request<Incoming>,
        path: &str,
    ) -> Response<Body> {
        let id = path.trim_start_matches("objects/shares/");
        if id.is_empty() || id.contains('/') {
            return json_error(StatusCode::BAD_REQUEST, "share link id is required");
        }
        let body = match read_json::<UpdateShareRequest>(request).await {
            Ok(body) => body,
            Err(_) => return json_error(StatusCode::BAD_REQUEST, "invalid share update payload"),
        };
        let existing = match self.store().get_share_link_by_id(id) {
            Ok(Some(record)) => record,
            Ok(None) => return json_error(StatusCode::NOT_FOUND, "share link not found"),
            Err(error) => return json_error(StatusCode::INTERNAL_SERVER_ERROR, &error.to_string()),
        };
        let manifest = match self
            .object_format
            .get_active_manifest(&existing.bucket, &existing.key)
        {
            Ok(Some(manifest)) => manifest,
            Ok(None) => return json_error(StatusCode::NOT_FOUND, "object not found"),
            Err(error) => return json_error(StatusCode::INTERNAL_SERVER_ERROR, &error.to_string()),
        };
        let requested_expiry = match expiry_from_seconds(body.expires_in_seconds) {
            Ok(value) => value,
            Err(message) => return json_error(StatusCode::BAD_REQUEST, &message),
        };
        let expires_at = match (requested_expiry, manifest.expires_at) {
            (Some(requested), Some(object)) => Some(requested.min(object)),
            (Some(requested), None) => Some(requested),
            (None, object) => object,
        };
        if expires_at.is_some_and(|value| value <= OffsetDateTime::now_utc()) {
            return json_error(
                StatusCode::BAD_REQUEST,
                "share expiry must be before object expiry",
            );
        }
        let updated = match self
            .object_format
            .update_share_link_expiry(id, expires_at.map(|value| value.unix_timestamp()))
        {
            Ok(Some(record)) => record,
            Ok(None) => return json_error(StatusCode::NOT_FOUND, "share link not found"),
            Err(error) => return json_error(StatusCode::INTERNAL_SERVER_ERROR, &error.to_string()),
        };
        json_response(StatusCode::OK, self.share_link_to_wire(&updated))
    }

    fn handle_revoke_share_link(&self, path: &str) -> Response<Body> {
        let id = path.trim_start_matches("objects/shares/");
        if id.is_empty() || id.contains('/') {
            return json_error(StatusCode::BAD_REQUEST, "share link id is required");
        }
        match self.object_format.revoke_share_link(id) {
            Ok(true) => json_response(StatusCode::OK, serde_json::json!({ "ok": true })),
            Ok(false) => json_error(StatusCode::NOT_FOUND, "share link not found"),
            Err(error) => json_error(StatusCode::INTERNAL_SERVER_ERROR, &error.to_string()),
        }
    }

    fn share_link_to_wire(&self, record: &crate::metadata::ShareLinkRecord) -> ShareLinkWire {
        let now = OffsetDateTime::now_utc().unix_timestamp();
        let expired = record.expires_at.is_some_and(|value| value <= now);
        let url = record
            .token_ciphertext
            .as_deref()
            .and_then(|ciphertext| self.object_format.reveal_share_token(ciphertext).ok())
            .map(|token| format!("/_public/{token}"));
        ShareLinkWire {
            id: record.id.clone(),
            url,
            description: if record.description.is_empty() {
                "No description provided".to_string()
            } else {
                record.description.clone()
            },
            created_at: rfc3339_unix(record.created_at),
            expires_at: record.expires_at.and_then(rfc3339_unix_opt),
            status: if expired {
                "expired".to_string()
            } else {
                "active".to_string()
            },
        }
    }

    /// Stream a file's bytes back to the browser, full or ranged (bounded RAM).
    ///
    /// Shares the exact chunk reader the S3 `get_object` path uses, so the
    /// bytes and checksum verification are identical. HEAD returns headers only.
    /// A directory path without a zero-byte marker has no object → 404.
    fn handle_content(
        &self,
        request: Request<Incoming>,
        _principal: &ResolvedPrincipal,
        is_head: bool,
    ) -> Response<Body> {
        let params = parse_list_params(request.uri().query().unwrap_or(""));
        let bucket = match params.get("bucket") {
            Some(value) if !value.is_empty() => value.clone(),
            _ => return json_error(StatusCode::BAD_REQUEST, "bucket is required"),
        };
        let key = match params.get("key") {
            Some(value) if !value.is_empty() => value.clone(),
            _ => return json_error(StatusCode::BAD_REQUEST, "key is required"),
        };
        if !is_safe_object_key(&key) {
            return json_error(
                StatusCode::BAD_REQUEST,
                "key must be a non-empty, relative, plain file path",
            );
        }
        let manifest = match self.object_format.get_active_manifest(&bucket, &key) {
            Ok(Some(manifest)) => manifest,
            Ok(None) => return json_error(StatusCode::NOT_FOUND, "object not found"),
            Err(error) => return json_error(StatusCode::INTERNAL_SERVER_ERROR, &error.to_string()),
        };
        if manifest.commit_state != crate::manifest::CommitState::Committed {
            return json_error(StatusCode::NOT_FOUND, "object not found");
        }
        let (range, content_range) =
            match parse_content_range(request.headers(), manifest.content_length) {
                Ok(parsed) => parsed,
                Err(unsatisfiable_length) => {
                    let mut response =
                        json_error(StatusCode::RANGE_NOT_SATISFIABLE, "range not satisfiable");
                    if let Ok(value) =
                        HeaderValue::from_str(&format!("bytes */{unsatisfiable_length}"))
                    {
                        response.headers_mut().insert(header::CONTENT_RANGE, value);
                    }
                    return response;
                }
            };
        let (status, content_length) = if content_range.is_some() {
            (StatusCode::PARTIAL_CONTENT, range.end - range.start)
        } else if range.start == 0 && range.end == manifest.content_length {
            (StatusCode::OK, manifest.content_length)
        } else {
            // A well-formed range that merely asked for the whole file edges
            // (e.g. "bytes=0-") still answers 200 without a Content-Range.
            (StatusCode::OK, range.end - range.start)
        };
        let spans = match ObjectFormatService::plan_read(&manifest, range) {
            Ok(plan) => plan.chunks,
            Err(_) => {
                return json_error(StatusCode::RANGE_NOT_SATISFIABLE, "range not satisfiable");
            }
        };
        let mut response = if is_head || spans.is_empty() {
            Response::new(Body::empty())
        } else {
            let stream = ObjectFormatService::read_spans_to_stream(
                Arc::clone(&self.object_format),
                &manifest,
                spans,
            );
            Response::new(Body::http_body_unsync(StreamBody::new(
                stream.map(|chunk| chunk.map(Frame::data)),
            )))
        };
        *response.status_mut() = status;
        if let Ok(value) = HeaderValue::from_str(&content_length.to_string()) {
            response.headers_mut().insert(header::CONTENT_LENGTH, value);
        }
        response.headers_mut().insert(
            header::CONTENT_TYPE,
            HeaderValue::from_str(&manifest.content_type)
                .unwrap_or(HeaderValue::from_static("application/octet-stream")),
        );
        if let Ok(value) = HeaderValue::from_str(&manifest.checksum.whole_object) {
            response.headers_mut().insert(header::ETAG, value);
        }
        let disposition = content_disposition(&basename_key(&key));
        if let Ok(value) = HeaderValue::from_str(&disposition) {
            response
                .headers_mut()
                .insert(header::CONTENT_DISPOSITION, value);
        }
        if let Some(value) = content_range
            && let Ok(header_value) = HeaderValue::from_str(&value)
        {
            response
                .headers_mut()
                .insert(header::CONTENT_RANGE, header_value);
        }
        response
            .headers_mut()
            .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
        response
    }

    // ---- telegram wizard ----------------------------------------------------

    async fn wizard_state(&self, _principal: &ResolvedPrincipal) -> Response<Body> {
        let driver = self.wizard_driver.lock().await;
        let snapshot = driver.snapshot();
        let health = self.telegram_health_snapshot().await;
        let authorized = driver.is_authorized()
            || matches!(health.status.session_state, SessionState::Authorized)
            || matches!(
                health.state,
                crate::telegram::TelegramConnectionState::Connected
            );
        json_response(
            StatusCode::OK,
            wizard_wire_value(
                snapshot.stage,
                authorized,
                snapshot.owner.as_deref(),
                None,
                matches!(
                    health.state,
                    crate::telegram::TelegramConnectionState::Connected
                ),
                &health,
            ),
        )
    }

    async fn wizard_begin(
        &self,
        request: Request<Incoming>,
        principal: &ResolvedPrincipal,
    ) -> Response<Body> {
        let Some(WizardBeginRequest {
            phone,
            flow_id,
            replace,
        }) = read_wizard_begin_request(request).await
        else {
            return json_error(StatusCode::BAD_REQUEST, "a login flow id is required");
        };
        let Some(flow_id) = flow_id.filter(|value| !value.trim().is_empty()) else {
            return json_error(StatusCode::BAD_REQUEST, "a login flow id is required");
        };
        let mut driver = self.wizard_driver.lock().await;
        if driver.is_authorized() {
            return json_response(
                StatusCode::OK,
                self.wizard_wire(
                    LoginStage::Authorized,
                    true,
                    None,
                    Some("already authorized"),
                )
                .await,
            );
        }
        let transport = match self.transport_manager.current().await {
            Ok(transport) => transport,
            Err(error) => {
                return driver_error_response(&LoginDriverError::Unauthorized(error.to_string()));
            }
        };
        let owner = &principal.user.username;
        let phone_for_confirmation = phone.clone();
        match driver
            .begin(&transport, phone, &flow_id, replace, owner)
            .await
        {
            Ok(step) => {
                if let Some(phone) = phone_for_confirmation.as_deref()
                    && let Err(error) = self.store().set_telegram_account_phone(phone)
                {
                    eprintln!("failed to store Telegram account phone: {error}");
                }
                if driver.is_authorized() {
                    self.finalize_wizard_success().await;
                }
                json_response(
                    StatusCode::OK,
                    self.wizard_wire(
                        step.stage,
                        driver.is_authorized(),
                        None,
                        Some(&step.message),
                    )
                    .await,
                )
            }
            Err(error) => driver_error_response(&error),
        }
    }

    async fn wizard_submit_code(
        &self,
        request: Request<Incoming>,
        principal: &ResolvedPrincipal,
    ) -> Response<Body> {
        let Some(WizardCodeRequest { code, flow_id }) = read_wizard_code_request(request).await
        else {
            return driver_error_response(&LoginDriverError::MissingCode);
        };
        let Some(flow_id) = flow_id.filter(|value| !value.trim().is_empty()) else {
            return driver_error_response(&LoginDriverError::FlowMismatch);
        };
        let mut driver = self.wizard_driver.lock().await;
        let transport = match self.transport_manager.current().await {
            Ok(transport) => transport,
            Err(error) => {
                return driver_error_response(&LoginDriverError::Unauthorized(error.to_string()));
            }
        };
        let Some(code) = code else {
            return driver_error_response(&LoginDriverError::MissingCode);
        };
        if driver.owner_name() != Some(principal.user.username.as_str()) {
            return driver_error_response(&LoginDriverError::FlowMismatch);
        }
        match driver.submit_code(&transport, &code, &flow_id).await {
            Ok(step) => {
                if driver.is_authorized() {
                    self.finalize_wizard_success().await;
                }
                json_response(
                    StatusCode::OK,
                    self.wizard_wire(
                        step.stage,
                        driver.is_authorized(),
                        None,
                        Some(&step.message),
                    )
                    .await,
                )
            }
            Err(error) => driver_error_response(&error),
        }
    }

    async fn wizard_submit_password(
        &self,
        request: Request<Incoming>,
        principal: &ResolvedPrincipal,
    ) -> Response<Body> {
        let Some(WizardPasswordRequest { password, flow_id }) =
            read_wizard_password_request(request).await
        else {
            return driver_error_response(&LoginDriverError::MissingPassword);
        };
        let Some(flow_id) = flow_id.filter(|value| !value.trim().is_empty()) else {
            return driver_error_response(&LoginDriverError::FlowMismatch);
        };
        let mut driver = self.wizard_driver.lock().await;
        let transport = match self.transport_manager.current().await {
            Ok(transport) => transport,
            Err(error) => {
                return driver_error_response(&LoginDriverError::Unauthorized(error.to_string()));
            }
        };
        let Some(password) = password else {
            return driver_error_response(&LoginDriverError::MissingPassword);
        };
        if driver.owner_name() != Some(principal.user.username.as_str()) {
            return driver_error_response(&LoginDriverError::FlowMismatch);
        }
        match driver
            .submit_password(&transport, &password, &flow_id)
            .await
        {
            Ok(step) => {
                if driver.is_authorized() {
                    self.finalize_wizard_success().await;
                }
                json_response(
                    StatusCode::OK,
                    self.wizard_wire(
                        step.stage,
                        driver.is_authorized(),
                        None,
                        Some(&step.message),
                    )
                    .await,
                )
            }
            Err(error) => driver_error_response(&error),
        }
    }

    async fn wizard_cancel(
        &self,
        request: Request<Incoming>,
        principal: &ResolvedPrincipal,
    ) -> Response<Body> {
        let flow_id = read_wizard_cancel_request(request)
            .await
            .and_then(|body| body.flow_id);
        let mut driver = self.wizard_driver.lock().await;
        match driver.cancel(flow_id.as_deref(), &principal.user.username) {
            Ok(()) => json_response(StatusCode::OK, serde_json::json!({ "ok": true })),
            Err(error) => driver_error_response(&error),
        }
    }

    /// After a successful phone/code/password login, reflect the now-authorised
    /// session in the stored status and hot-swap the live transport so later
    /// object operations use the refreshed Telegram session immediately.
    async fn finalize_wizard_success(&self) {
        let _ = self.transport_manager.refresh().await;
    }

    async fn wizard_wire(
        &self,
        stage: LoginStage,
        authorized: bool,
        owner: Option<&str>,
        message: Option<&str>,
    ) -> serde_json::Value {
        let health = self.telegram_health_snapshot().await;
        wizard_wire_value(
            stage,
            authorized,
            owner,
            message,
            matches!(
                health.state,
                crate::telegram::TelegramConnectionState::Connected
            ),
            &health,
        )
    }

    async fn telegram_settings(&self, _principal: &ResolvedPrincipal) -> Response<Body> {
        json_response(
            StatusCode::OK,
            serde_json::json!({ "settings": self.telegram_settings_wire() }),
        )
    }

    async fn telegram_save_settings(
        &self,
        request: Request<Incoming>,
        _principal: &ResolvedPrincipal,
    ) -> Response<Body> {
        let TelegramSettingsRequest {
            telegram_api_id,
            telegram_api_hash,
            telegram_storage_chat_id,
            telegram_proxy_url,
            telegram_proxy_username,
            telegram_proxy_password,
            telegram_proxy_mode,
        } = match read_json::<TelegramSettingsRequest>(request).await {
            Ok(body) => body,
            Err(_) => {
                return json_error(StatusCode::BAD_REQUEST, "invalid telegram settings payload");
            }
        };

        let current = self
            .store()
            .telegram_bootstrap_settings()
            .unwrap_or_default()
            .unwrap_or_default();
        let next = TelegramBootstrapSettings {
            telegram_api_id: clean_required(telegram_api_id).or(current.telegram_api_id),
            telegram_api_hash: clean_required(telegram_api_hash).or(current.telegram_api_hash),
            telegram_session_path: None,
            telegram_storage_chat_id: clean_required(telegram_storage_chat_id)
                .or(current.telegram_storage_chat_id),
            telegram_proxy_url: clean_optional(telegram_proxy_url),
            telegram_proxy_username: clean_optional(telegram_proxy_username),
            telegram_proxy_password: clean_optional(telegram_proxy_password),
            telegram_proxy_mode: clean_optional(telegram_proxy_mode)
                .or(current.telegram_proxy_mode)
                .or_else(|| Some("auto".to_string())),
        };
        if next.telegram_api_id.as_deref().is_none_or(str::is_empty)
            || next.telegram_api_hash.as_deref().is_none_or(str::is_empty)
            || next
                .telegram_storage_chat_id
                .as_deref()
                .is_none_or(str::is_empty)
        {
            return json_error(
                StatusCode::BAD_REQUEST,
                "telegram api id, api hash, and storage chat id are required",
            );
        }
        if let Err(error) = validate_telegram_bootstrap_settings(&next) {
            return json_error(StatusCode::BAD_REQUEST, &error.to_string());
        }
        let normalized_storage_chat_id =
            match normalize_telegram_storage_chat_id(next.telegram_storage_chat_id.as_deref()) {
                Ok(value) => value,
                Err(error) => return json_error(StatusCode::BAD_REQUEST, &error.to_string()),
            };
        let mut next = next;
        next.telegram_storage_chat_id = Some(normalized_storage_chat_id.clone());
        if let Err(error) = self.store().set_telegram_bootstrap_settings(&next) {
            return json_error(StatusCode::INTERNAL_SERVER_ERROR, &error.to_string());
        }
        self.object_format
            .set_storage_chat_id(normalized_storage_chat_id);
        let refresh_error = match AssertUnwindSafe(self.transport_manager.refresh())
            .catch_unwind()
            .await
        {
            Ok(Ok(_)) => None,
            Ok(Err(error)) => Some(error.to_string()),
            Err(_) => Some("telegram transport refresh failed".to_string()),
        };

        json_response(
            StatusCode::OK,
            serde_json::json!({
                "settings": self.telegram_settings_wire(),
                "refresh_error": refresh_error,
            }),
        )
    }

    async fn telegram_disconnect(&self, request: Request<Incoming>) -> Response<Body> {
        let TelegramDisconnectRequest {
            delete_uploaded_files,
            phone_confirmation,
        } = match read_json::<TelegramDisconnectRequest>(request).await {
            Ok(body) => body,
            Err(_) => {
                return json_error(
                    StatusCode::BAD_REQUEST,
                    "invalid connection removal payload",
                );
            }
        };
        if phone_confirmation.trim().is_empty() {
            return json_error(
                StatusCode::BAD_REQUEST,
                "enter the phone number linked to this Telegram account",
            );
        }
        match self
            .store()
            .telegram_account_phone_matches(&phone_confirmation)
        {
            Ok(true) => {}
            Ok(false) => {
                return json_error(
                    StatusCode::FORBIDDEN,
                    "the phone number does not match the connected Telegram account",
                );
            }
            Err(error) => return json_error(StatusCode::INTERNAL_SERVER_ERROR, &error.to_string()),
        }
        let job = match self.store().begin_connection_removal(delete_uploaded_files) {
            Ok(job) => job,
            Err(crate::metadata::MetadataError::ConnectionRemovalInProgress) => {
                return json_error(
                    StatusCode::CONFLICT,
                    "a connection removal is already in progress",
                );
            }
            Err(error) => return json_error(StatusCode::INTERNAL_SERVER_ERROR, &error.to_string()),
        };
        self.object_format.clear_recovery_snapshot();
        self.object_format.ensure_workers();
        json_response(
            StatusCode::ACCEPTED,
            serde_json::json!({
                "ok": true,
                "job": job,
                "message": if delete_uploaded_files {
                    "Connection removed; uploaded Telegram files are queued for worker cleanup."
                } else {
                    "Connection removed; uploaded Telegram files were left in Telegram."
                }
            }),
        )
    }

    async fn telegram_storage_settings(&self) -> Response<Body> {
        json_response(StatusCode::OK, self.telegram_storage_settings_wire())
    }

    async fn telegram_save_storage_settings(&self, request: Request<Incoming>) -> Response<Body> {
        let StorageSettingsRequest { chunk_size } = match read_json(request).await {
            Ok(body) => body,
            Err(_) => {
                return json_error(StatusCode::BAD_REQUEST, "invalid storage settings payload");
            }
        };
        let Some(chunk_size) = chunk_size else {
            return json_error(StatusCode::BAD_REQUEST, "chunk_size is required");
        };
        if let Err(error) = crate::config::AppConfig::validate_chunk_size(chunk_size) {
            return json_error(StatusCode::BAD_REQUEST, &error.to_string());
        }
        if let Err(error) = self.store().set_telegram_chunk_size(chunk_size) {
            return json_error(StatusCode::INTERNAL_SERVER_ERROR, &error.to_string());
        }
        if let Err(error) = self.object_format.set_chunk_size(chunk_size) {
            return json_error(StatusCode::INTERNAL_SERVER_ERROR, &error.to_string());
        }
        json_response(StatusCode::OK, self.telegram_storage_settings_wire())
    }

    fn telegram_storage_settings_wire(&self) -> StorageSettingsWire {
        StorageSettingsWire {
            chunk_size: self.object_format.chunk_size(),
            min_chunk_size: crate::config::MIN_CHUNK_SIZE,
            max_chunk_size: crate::config::MAX_CHUNK_SIZE,
            source: "database".to_string(),
        }
    }

    fn telegram_settings_wire(&self) -> TelegramSettingsWire {
        let stored = self
            .store()
            .telegram_bootstrap_settings()
            .ok()
            .flatten()
            .unwrap_or_default();
        let telegram_account_phone = self.store().telegram_account_phone().ok().flatten();
        let resolved = self.config.resolve_telegram_bootstrap(self.store()).ok();
        TelegramSettingsWire {
            telegram_api_id: resolved
                .as_ref()
                .map(|settings| settings.telegram_api_id.clone())
                .or_else(|| stored.telegram_api_id.clone())
                .unwrap_or_default(),
            telegram_api_hash: resolved
                .as_ref()
                .map(|settings| settings.telegram_api_hash.clone())
                .or_else(|| stored.telegram_api_hash.clone())
                .unwrap_or_default(),
            telegram_storage_chat_id: resolved
                .as_ref()
                .map(|settings| settings.telegram_storage_chat_id.clone())
                .or_else(|| stored.telegram_storage_chat_id.clone())
                .unwrap_or_default(),
            telegram_proxy_url: resolved
                .as_ref()
                .and_then(|settings| settings.telegram_proxy_url.clone())
                .or_else(|| stored.telegram_proxy_url.clone())
                .unwrap_or_default(),
            telegram_proxy_username: resolved
                .as_ref()
                .and_then(|settings| settings.telegram_proxy_username.clone())
                .or_else(|| stored.telegram_proxy_username.clone())
                .unwrap_or_default(),
            telegram_proxy_password: resolved
                .as_ref()
                .and_then(|settings| settings.telegram_proxy_password.clone())
                .or_else(|| stored.telegram_proxy_password.clone())
                .unwrap_or_default(),
            telegram_proxy_mode: resolved
                .as_ref()
                .map(|settings| settings.telegram_proxy_mode.clone())
                .or_else(|| stored.telegram_proxy_mode.clone())
                .unwrap_or_else(|| "auto".to_string()),
            telegram_account_phone,
        }
    }

    // ---- binary content write (upload) ---------------------------------------

    /// Stream a single file's bytes into the store, chunk-by-chunk (bounded RAM).
    /// Transport bridge only: reuses the S3 data-plane writer with the raw request
    /// body as the inbound stream. The key may be a full path (`dir/sub/name.ext`).
    async fn handle_upload_content(
        &self,
        request: Request<Incoming>,
        _principal: &ResolvedPrincipal,
    ) -> Response<Body> {
        let mut params = request
            .uri()
            .query()
            .map(parse_list_params)
            .unwrap_or_default();
        let bucket = request
            .uri()
            .query()
            .map(parse_list_params)
            .and_then(|mut query| query.remove("bucket"))
            .filter(|value| !value.is_empty());
        let Some(bucket) = bucket else {
            return json_error(StatusCode::BAD_REQUEST, "bucket is required");
        };
        if !matches!(self.object_format.bucket_exists(&bucket), Ok(true)) {
            return json_error(StatusCode::NOT_FOUND, "bucket not found");
        }
        let key = request
            .uri()
            .query()
            .map(parse_list_params)
            .and_then(|mut query| query.remove("key"))
            .unwrap_or_default();
        if !is_safe_object_key(&key) {
            return json_error(
                StatusCode::BAD_REQUEST,
                "key must be a non-empty, relative, plain file path",
            );
        }
        let content_type = request
            .headers()
            .get(header::CONTENT_TYPE)
            .and_then(|value| value.to_str().ok())
            .filter(|value| !value.is_empty())
            .unwrap_or("application/octet-stream")
            .to_string();
        let expires_in_seconds = params
            .remove("expires_in_seconds")
            .map(|value| value.parse::<u64>())
            .transpose()
            .map_err(|_| "invalid expiry seconds".to_string());
        let expires_at = match expires_in_seconds {
            Ok(value) => match expiry_from_seconds(value) {
                Ok(value) => value,
                Err(message) => return json_error(StatusCode::BAD_REQUEST, &message),
            },
            Err(message) => return json_error(StatusCode::BAD_REQUEST, &message),
        };
        let body = body_to_streaming_blob(request.into_body());
        let manifest = match self
            .object_format
            .put_stream_with_expiry(&bucket, &key, &content_type, Some(body), None, expires_at)
            .await
        {
            Ok(manifest) => manifest,
            Err(error) => {
                return json_error(StatusCode::INTERNAL_SERVER_ERROR, &error.to_string());
            }
        };
        json_response(
            StatusCode::OK,
            serde_json::json!({
                "size": manifest.content_length,
                "etag": manifest.checksum.whole_object,
                "version_id": manifest.object_id,
            }),
        )
    }

    async fn handle_overview(&self, principal: &ResolvedPrincipal) -> Response<Body> {
        let metadata_status = self
            .object_format
            .metadata_status()
            .unwrap_or_else(|_| empty_meta());
        let object_status = self
            .object_format
            .status()
            .unwrap_or_else(|_| empty_object());
        let durable = self.object_format.durable_metrics().unwrap_or_default();
        let acknowledgements = self
            .object_format
            .metadata_store()
            .recovery_acknowledgements()
            .unwrap_or_default();
        let recovery = match self.object_format.cached_recovery_snapshot() {
            Ok(snapshot) => RecoveryWire::from_snapshot(snapshot, &acknowledgements),
            Err(error) => RecoveryWire::failed(error),
        };
        let connection_removal = self
            .object_format
            .metadata_store()
            .connection_removal_job()
            .ok()
            .flatten();
        let health = self.telegram_health_snapshot().await;
        let session_state = health.status.session_state.clone();
        let session_state_debug = format!("{session_state:?}");
        let session_usable = telegram_session_usable(session_state.clone());
        let telegram = TelegramStateWire {
            session_state: session_state_debug.clone(),
            connection_state: telegram_connection_state_label(&health.state).to_string(),
            detail: health.detail.clone(),
            storage_chat_id: Some(health.status.storage_chat_id.clone()),
        };
        let storage = StorageWire {
            metadata_path: redact_path(&self.config.metadata_path().display().to_string()),
            data_dir: redact_path(&self.config.data_dir().display().to_string()),
            buckets: metadata_status.buckets,
            committed_objects: metadata_status.committed_objects,
            active_objects: metadata_status.active_objects,
            staged_objects: metadata_status.staged_objects,
            recovery_markers: metadata_status.recovery_markers,
            chunk_size: object_status.chunk_size,
            recovery_required_objects: object_status.recovery_required_objects,
        };
        let checks = vec![
            check("Telegram storage", session_usable, &health.detail),
            check(
                "Storage chat",
                !health.status.storage_chat_id.is_empty(),
                "resolved from Telegram bootstrap settings",
            ),
            check(
                "UI assets",
                self.ui_dist_dir.join("index.html").exists(),
                "Svelte build output present",
            ),
        ];
        json_response(
            StatusCode::OK,
            serde_json::json!({
                "checked_at": OffsetDateTime::from_unix_timestamp(health.checked_at)
                    .map(rfc3339)
                    .unwrap_or_else(|_| rfc3339(OffsetDateTime::now_utc())),
                "telegram_last_success_at": health.last_success_at.and_then(|value|
                    OffsetDateTime::from_unix_timestamp(value).ok().map(rfc3339)),
                "session": {"authenticated": true, "user": UserWire::from_user(&principal.user)},
                "storage": storage,
                "transfers": durable,
                "recovery": recovery,
                "telegram": telegram,
                "connection_removal": connection_removal,
                "checks": checks,
            }),
        )
    }

    async fn handle_recovery_repair(&self) -> Response<Body> {
        match self.object_format.reconcile().await {
            Ok(report) => json_response(
                StatusCode::OK,
                serde_json::json!({
                    "ok": true,
                    "report": report,
                }),
            ),
            Err(error) => json_error(StatusCode::INTERNAL_SERVER_ERROR, &error.to_string()),
        }
    }

    /// Add or remove operator acknowledgements for recovery issues.
    ///
    /// Acknowledgements are stored by fingerprint and pruned on every write to the
    /// set of issues the latest scan still reports, so resolved issues cannot leave
    /// entries behind in `app_settings`.
    async fn handle_recovery_acknowledge(
        &self,
        request: Request<Incoming>,
        principal: &ResolvedPrincipal,
        acknowledge: bool,
    ) -> Response<Body> {
        let RecoveryAcknowledgeRequest { ids } =
            match read_json::<RecoveryAcknowledgeRequest>(request).await {
                Ok(body) => body,
                Err(_) => {
                    return json_error(StatusCode::BAD_REQUEST, "invalid acknowledgement payload");
                }
            };
        if ids.is_empty() {
            return json_error(StatusCode::BAD_REQUEST, "at least one issue id is required");
        }

        let live: std::collections::HashSet<String> =
            match self.object_format.cached_recovery_snapshot() {
                Ok((_, issues, _)) => issues.iter().map(|issue| issue.fingerprint()).collect(),
                Err(error) => return json_error(StatusCode::INTERNAL_SERVER_ERROR, &error),
            };
        if let Some(unknown) = ids.iter().find(|id| !live.contains(*id)) {
            return json_error(
                StatusCode::NOT_FOUND,
                &format!("unknown recovery issue id {unknown}"),
            );
        }

        let store = self.object_format.metadata_store();
        let mut acknowledgements = match store.recovery_acknowledgements() {
            Ok(existing) => existing,
            Err(error) => {
                return json_error(StatusCode::INTERNAL_SERVER_ERROR, &error.to_string());
            }
        };
        // Drop acknowledgements for issues the scan no longer reports.
        acknowledgements.retain(|id, _| live.contains(id));
        if acknowledge {
            let at = rfc3339(OffsetDateTime::now_utc());
            for id in &ids {
                acknowledgements.insert(
                    id.clone(),
                    RecoveryAck {
                        at: at.clone(),
                        by: principal.user.username.clone(),
                    },
                );
            }
        } else {
            for id in &ids {
                acknowledgements.remove(id);
            }
        }
        if let Err(error) = store.set_recovery_acknowledgements(&acknowledgements) {
            return json_error(StatusCode::INTERNAL_SERVER_ERROR, &error.to_string());
        }

        json_response(
            StatusCode::OK,
            serde_json::json!({
                "ok": true,
                "acknowledged_count": acknowledgements.len(),
                "unacknowledged_count": live.len().saturating_sub(acknowledgements.len()),
            }),
        )
    }

    // ---- static (SPA) serving ------------------------------------------------

    async fn handle_static(self: Arc<Self>, request: Request<Incoming>) -> Response<Body> {
        if !matches!(request.method(), &Method::GET | &Method::HEAD) {
            return json_error(StatusCode::METHOD_NOT_ALLOWED, "method not allowed");
        }
        let path = request.uri().path();
        if let Some(asset_path) = path.strip_prefix(ADMIN_ASSET_PREFIX) {
            let Some(file_path) = safe_join(&self.ui_dist_dir().join("assets"), asset_path) else {
                return json_error(StatusCode::NOT_FOUND, "asset not found");
            };
            return match read_static_file(&file_path).await {
                Ok(Some(response)) => response,
                Ok(None) => json_error(StatusCode::NOT_FOUND, "asset not found"),
                Err(_) => json_error(
                    StatusCode::SERVICE_UNAVAILABLE,
                    "admin ui assets unavailable",
                ),
            };
        }
        let index = self.ui_dist_dir().join("index.html");
        match read_static_file(&index).await {
            Ok(Some(response)) => response,
            Ok(None) => json_error(StatusCode::SERVICE_UNAVAILABLE, "admin ui assets missing"),
            Err(_) => json_error(
                StatusCode::SERVICE_UNAVAILABLE,
                "admin ui assets unavailable",
            ),
        }
    }
}

// ---- helpers used by the admin implementation -------------------------------

fn telegram_session_usable(state: SessionState) -> bool {
    matches!(state, SessionState::Authorized | SessionState::Reused)
}

fn telegram_connection_state_label(
    state: &crate::telegram::TelegramConnectionState,
) -> &'static str {
    match state {
        crate::telegram::TelegramConnectionState::Connected => "connected",
        crate::telegram::TelegramConnectionState::Disconnected => "disconnected",
        crate::telegram::TelegramConnectionState::NeedsReauth => "needs_reauth",
        crate::telegram::TelegramConnectionState::NotConfigured => "not_configured",
    }
}

fn clean_required(value: Option<String>) -> Option<String> {
    value
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

fn clean_optional(value: Option<String>) -> Option<String> {
    value
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

// ---- binary content + telegram wizard helpers ------------------------------

/// Parse an optional HTTP `Range: bytes=…` header against a known content
/// length. Returns `(exclusive Range, optional `Content-Range` value)` on a
/// satisfiable request, or `Err(content_length)` when the requested range does
/// not overlap the object (used to answer `416`).
fn parse_content_range(
    headers: &http::HeaderMap,
    content_length: u64,
) -> Result<(std::ops::Range<u64>, Option<String>), u64> {
    let value = headers
        .get(header::RANGE)
        .and_then(|value| value.to_str().ok());
    let Some(value) = value else {
        return Ok((0..content_length, None));
    };
    let value = match value.strip_prefix("bytes=") {
        Some(value) => value,
        None => return Err(content_length),
    };
    let Some((start_text, end_text)) = value.split_once('-') else {
        return Err(content_length);
    };
    if content_length == 0 {
        return Err(content_length);
    }
    let (start, end) = if start_text.is_empty() {
        // Suffix range `bytes=-N`: last N bytes.
        let suffix = match end_text.parse::<u64>() {
            Ok(value) => value.min(content_length),
            Err(_) => return Err(content_length),
        };
        let start = content_length.saturating_sub(suffix);
        (start, content_length)
    } else {
        let start = match start_text.parse::<u64>() {
            Ok(value) => value,
            Err(_) => return Err(content_length),
        };
        let end = if end_text.is_empty() {
            content_length
        } else {
            match end_text.parse::<u64>() {
                Ok(value) => value.saturating_add(1).min(content_length),
                Err(_) => return Err(content_length),
            }
        };
        (start, end)
    };
    if start >= end || start >= content_length {
        return Err(content_length);
    }
    let content_range = format!("bytes {}-{}/{}", start, end - 1, content_length);
    Ok((start..end, Some(content_range)))
}

/// `Content-Disposition: attachment; filename="…"` with an ASCII fallback and
/// RFC 5987 percent-encoding for non-ASCII names.
pub(crate) fn content_disposition(basename: &str) -> String {
    if basename.is_ascii() && !basename.contains(['"', '\\', '\r', '\n']) {
        return format!("attachment; filename=\"{basename}\"");
    }
    let encoded = percent_encode_filename(basename);
    format!("attachment; filename=\"download\"; filename*=UTF-8''{encoded}")
}

fn percent_encode_filename(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for byte in value.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'.' | b'_' | b'~') {
            out.push(byte as char);
        } else {
            out.push_str(&format!("%{byte:02X}"));
        }
    }
    out
}

fn wizard_wire_value(
    stage: LoginStage,
    authorized: bool,
    owner: Option<&str>,
    message: Option<&str>,
    connection_ready: bool,
    health: &TelegramConnectionHealth,
) -> serde_json::Value {
    let message = if authorized && !connection_ready {
        Some(format!(
            "{} Storage connection is not ready: {}",
            message.unwrap_or("Telegram account authorized."),
            health.detail
        ))
    } else {
        message.map(str::to_string)
    };
    serde_json::json!({
        "phase": login_stage_name(&stage),
        "needs_2fa": stage == LoginStage::TwoFa,
        "authorized": authorized,
        "owner": owner,
        "message": message,
        "connection_ready": connection_ready,
        "connection_state": telegram_connection_state_label(&health.state),
        "health_detail": health.detail,
    })
}

fn login_stage_name(stage: &LoginStage) -> &'static str {
    match stage {
        LoginStage::Idle => "idle",
        LoginStage::Code => "code",
        LoginStage::TwoFa => "two_fa",
        LoginStage::Authorized => "authorized",
    }
}

fn driver_error_response(error: &LoginDriverError) -> Response<Body> {
    let (status, message) = match error {
        LoginDriverError::Occupied { stage, owner } => {
            let hint = owner.as_deref().unwrap_or("another operator");
            (
                StatusCode::CONFLICT,
                format!(
                    "a Telegram login by {hint} is already in progress at {:?}",
                    login_stage_name(stage)
                ),
            )
        }
        LoginDriverError::FlowMismatch => (
            StatusCode::CONFLICT,
            "that Telegram login flow is no longer active".to_string(),
        ),
        LoginDriverError::MissingPhone => (
            StatusCode::BAD_REQUEST,
            "a phone number is required".to_string(),
        ),
        LoginDriverError::MissingCode => (
            StatusCode::BAD_REQUEST,
            "the confirmation code is required".to_string(),
        ),
        LoginDriverError::MissingPassword => (
            StatusCode::BAD_REQUEST,
            "the cloud password is required".to_string(),
        ),
        LoginDriverError::InvalidCode => (
            StatusCode::BAD_REQUEST,
            "that confirmation code is not valid".to_string(),
        ),
        LoginDriverError::ExpiredCode => (
            StatusCode::GONE,
            "that confirmation code has expired".to_string(),
        ),
        LoginDriverError::WrongPassword => (
            StatusCode::BAD_REQUEST,
            "that cloud password is not correct".to_string(),
        ),
        LoginDriverError::SignUpRequired => (
            StatusCode::BAD_REQUEST,
            "this account requires sign-up using Telegram's official app".to_string(),
        ),
        LoginDriverError::Unauthorized(detail) => (
            StatusCode::BAD_REQUEST,
            format!("Telegram login failed: {detail}"),
        ),
    };
    json_error(status, &message)
}

#[derive(Debug, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
struct WizardBeginRequest {
    #[serde(default)]
    phone: Option<String>,
    flow_id: Option<String>,
    #[serde(default)]
    replace: bool,
}

#[derive(Debug, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
struct WizardCodeRequest {
    code: Option<String>,
    flow_id: Option<String>,
}

#[derive(Debug, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
struct WizardPasswordRequest {
    password: Option<String>,
    flow_id: Option<String>,
}

#[derive(Debug, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
struct WizardCancelRequest {
    flow_id: Option<String>,
}

async fn read_json_body_opt<T: serde::de::DeserializeOwned>(
    request: Request<Incoming>,
) -> Option<T> {
    let body = match request.into_body().collect().await {
        Ok(value) => value.to_bytes(),
        Err(_) => return None,
    };
    serde_json::from_slice(&body).ok()
}

async fn read_wizard_begin_request(request: Request<Incoming>) -> Option<WizardBeginRequest> {
    read_json_body_opt(request).await
}

async fn read_wizard_code_request(request: Request<Incoming>) -> Option<WizardCodeRequest> {
    read_json_body_opt(request).await
}

async fn read_wizard_password_request(request: Request<Incoming>) -> Option<WizardPasswordRequest> {
    read_json_body_opt(request).await
}

async fn read_wizard_cancel_request(request: Request<Incoming>) -> Option<WizardCancelRequest> {
    read_json_body_opt(request).await
}

fn object_to_wire(manifest: &ObjectManifest, key: &str, shared_links: u64) -> ObjectEntryWire {
    ObjectEntryWire {
        name: basename_key(key),
        key: key.to_string(),
        size: manifest.content_length,
        last_modified: rfc3339(manifest.created_at),
        etag: manifest.checksum.whole_object.clone(),
        expires_at: manifest.expires_at.and_then(|value| {
            value
                .format(&time::format_description::well_known::Rfc3339)
                .ok()
        }),
        shared_links,
    }
}

pub(crate) fn basename_key(key: &str) -> String {
    key.rsplit('/')
        .next()
        .filter(|part| !part.is_empty())
        .unwrap_or(key)
        .to_string()
}

fn is_safe_folder_path(path: &str) -> bool {
    if path.starts_with('/') || path.contains("..") || path.contains('\u{0}') {
        return false;
    }
    path.len() <= 1024
}

fn is_safe_object_key(key: &str) -> bool {
    if key.is_empty()
        || key.starts_with('/')
        || key.ends_with('/')
        || key.contains('\u{0}')
        || key.len() > 2048
    {
        return false;
    }
    // Reject any `..` path traversal (including within filename segments).
    !key.split(['/', '\\']).any(|segment| segment == "..")
}

fn body_to_streaming_blob(request_body: Incoming) -> StreamingBlob {
    use http_body_util::BodyExt as _;
    // Each item is Result<Bytes, hyper::Error>; hyper::Error: std::error::Error.
    StreamingBlob::wrap(request_body.into_data_stream())
}

fn parse_list_params(query: &str) -> std::collections::HashMap<String, String> {
    use url::form_urlencoded;
    form_urlencoded::parse(query.as_bytes())
        .into_owned()
        .collect()
}

fn session_anonymous() -> Response<Body> {
    json_response(
        StatusCode::OK,
        SessionResponse {
            authenticated: false,
            user: None,
            issued_at: None,
            expires_at: None,
            csrf_token: None,
        },
    )
}

fn mask_auth_error(error: AuthError) -> AuthError {
    match error.kind {
        auth::AuthErrorKind::InvalidPassword | auth::AuthErrorKind::UnknownUser => AuthError {
            kind: auth::AuthErrorKind::InvalidPassword,
            message: "invalid username or password".to_string(),
            retry_after_secs: None,
        },
        _ => error,
    }
}

fn auth_error_response(error: &AuthError) -> Response<Body> {
    let status = error.http_status();
    let retry_header = error
        .retry_after_secs
        .and_then(|secs| HeaderValue::from_str(&secs.to_string()).ok());
    let message = match error.kind {
        auth::AuthErrorKind::RateLimited | auth::AuthErrorKind::LockedOut => {
            match error.retry_after_secs {
                Some(secs) => format!("{} (retry after {}s)", error.message, secs),
                None => error.message.clone(),
            }
        }
        _ => error.message.clone(),
    };
    let mut response = json_error(status, &message);
    if let Some(value) = retry_header {
        response.headers_mut().insert("retry-after", value);
    }
    response
}

fn bucket_error_response(error: &crate::object_format::ObjectFormatError) -> Response<Body> {
    match error {
        crate::object_format::ObjectFormatError::Metadata(
            crate::metadata::MetadataError::BucketAlreadyExists(name),
        ) => json_error(
            StatusCode::CONFLICT,
            &format!("bucket already exists: {name}"),
        ),
        crate::object_format::ObjectFormatError::Metadata(
            crate::metadata::MetadataError::BucketNotFound(name),
        ) => json_error(StatusCode::NOT_FOUND, &format!("bucket not found: {name}")),
        crate::object_format::ObjectFormatError::Metadata(
            crate::metadata::MetadataError::BucketNotEmpty(name),
        ) => json_error(StatusCode::CONFLICT, &format!("bucket not empty: {name}")),
        crate::object_format::ObjectFormatError::Metadata(
            crate::metadata::MetadataError::InvalidManifest(message),
        )
        | crate::object_format::ObjectFormatError::InvalidPlan(message) => {
            json_error(StatusCode::BAD_REQUEST, message)
        }
        _ => json_error(StatusCode::INTERNAL_SERVER_ERROR, &error.to_string()),
    }
}

fn internal_auth(message: String) -> AuthError {
    AuthError {
        kind: auth::AuthErrorKind::Internal,
        message,
        retry_after_secs: None,
    }
}

fn empty_meta() -> crate::metadata::MetadataStatus {
    crate::metadata::MetadataStatus {
        path: None,
        schema_version: 0,
        buckets: 0,
        committed_objects: 0,
        active_objects: 0,
        staged_objects: 0,
        recovery_markers: 0,
    }
}

fn empty_object() -> crate::object_format::ObjectFormatStatus {
    crate::object_format::ObjectFormatStatus {
        data_dir: PathBuf::new(),
        chunk_size: 0,
        committed_objects: 0,
        staged_objects: 0,
        recovery_required_objects: 0,
        orphaned_chunks: 0,
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "snake_case")]
struct CheckItemWire {
    label: String,
    ok: bool,
    detail: String,
}

fn check(label: &str, ok: bool, detail: &str) -> CheckItemWire {
    CheckItemWire {
        label: label.to_string(),
        ok,
        detail: detail.to_string(),
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "snake_case")]
struct StorageWire {
    metadata_path: String,
    data_dir: String,
    buckets: u64,
    committed_objects: u64,
    active_objects: u64,
    staged_objects: u64,
    recovery_markers: u64,
    chunk_size: u64,
    recovery_required_objects: u64,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "snake_case")]
struct RecoveryIssueWire {
    /// Stable fingerprint, used by the console to acknowledge this issue.
    id: String,
    object_id: Option<String>,
    bucket: Option<String>,
    key: Option<String>,
    path: Option<String>,
    commit_state: Option<String>,
    kind: String,
    summary: String,
    details: Vec<String>,
    acknowledged_at: Option<String>,
    acknowledged_by: Option<String>,
}

impl From<RecoveryIssueModel> for RecoveryIssueWire {
    fn from(value: RecoveryIssueModel) -> Self {
        Self {
            id: value.fingerprint(),
            object_id: value.object_id.map(|id| id.to_string()),
            bucket: value.bucket,
            key: value.key,
            path: value.path,
            commit_state: value.commit_state.map(|state| state.as_str().to_string()),
            kind: value.kind,
            summary: value.summary,
            details: value.details,
            acknowledged_at: None,
            acknowledged_by: None,
        }
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "snake_case")]
struct RecoveryWire {
    checked_at: Option<String>,
    /// Every issue the scan found, acknowledged or not.
    issue_count: u64,
    /// The actionable subset the console leads with.
    unacknowledged_count: u64,
    scan_ok: bool,
    scan_error: Option<String>,
    issues: Vec<RecoveryIssueWire>,
}

impl RecoveryWire {
    fn from_snapshot(
        snapshot: crate::object_format::RecoverySnapshot,
        acknowledgements: &RecoveryAcknowledgements,
    ) -> Self {
        let (checked_at, issues, scan_error) = snapshot;
        let issues: Vec<RecoveryIssueWire> = issues
            .into_iter()
            .map(|issue| {
                let mut wire = RecoveryIssueWire::from(issue);
                if let Some(ack) = acknowledgements.get(&wire.id) {
                    wire.acknowledged_at = Some(ack.at.clone());
                    wire.acknowledged_by = Some(ack.by.clone());
                }
                wire
            })
            .collect();
        Self {
            issue_count: issues.len() as u64,
            unacknowledged_count: issues
                .iter()
                .filter(|issue| issue.acknowledged_at.is_none())
                .count() as u64,
            scan_ok: scan_error.is_none(),
            scan_error,
            checked_at: checked_at
                .and_then(|value| OffsetDateTime::from_unix_timestamp(value).ok().map(rfc3339)),
            issues,
        }
    }

    fn failed(error: String) -> Self {
        Self {
            issue_count: 0,
            unacknowledged_count: 0,
            scan_ok: false,
            scan_error: Some(error),
            checked_at: None,
            issues: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "snake_case")]
struct TelegramStateWire {
    session_state: String,
    connection_state: String,
    detail: String,
    storage_chat_id: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "snake_case")]
struct TelegramSettingsWire {
    telegram_api_id: String,
    telegram_api_hash: String,
    telegram_storage_chat_id: String,
    telegram_proxy_url: String,
    telegram_proxy_username: String,
    telegram_proxy_password: String,
    telegram_proxy_mode: String,
    telegram_account_phone: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "snake_case")]
struct StorageSettingsRequest {
    chunk_size: Option<u64>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "snake_case")]
struct StorageSettingsWire {
    chunk_size: u64,
    min_chunk_size: u64,
    max_chunk_size: u64,
    source: String,
}

// ---- cookie / csrf primitives -----------------------------------------------

fn read_session_cookie(headers: &http::HeaderMap) -> Option<String> {
    let cookie_header = headers.get(header::COOKIE)?.to_str().ok()?;
    parse_cookie(cookie_header, ADMIN_SESSION_COOKIE).map(ToString::to_string)
}

fn parse_cookie<'a>(cookie_header: &'a str, name: &str) -> Option<&'a str> {
    cookie_header.split(';').map(str::trim).find_map(|pair| {
        pair.split_once('=')
            .and_then(|(key, value)| (key.trim() == name).then_some(value))
    })
}

fn sign_payload(payload: &[u8], secret: &[u8]) -> String {
    let key = hmac::Key::new(hmac::HMAC_SHA256, secret);
    let signature = hmac::sign(&key, payload);
    format!(
        "{}.{}",
        URL_SAFE_NO_PAD.encode(payload),
        URL_SAFE_NO_PAD.encode(signature.as_ref())
    )
}

fn decode_claims(value: &str, secret: &[u8]) -> Result<SessionClaims, ()> {
    let (payload, signature) = value.split_once('.').ok_or(())?;
    let payload_bytes = URL_SAFE_NO_PAD.decode(payload).map_err(|_| ())?;
    let signature_bytes = URL_SAFE_NO_PAD.decode(signature).map_err(|_| ())?;
    let key = hmac::Key::new(hmac::HMAC_SHA256, secret);
    hmac::verify(&key, &payload_bytes, &signature_bytes).map_err(|_| ())?;
    serde_json::from_slice::<SessionClaims>(&payload_bytes).map_err(|_| ())
}

fn require_csrf(request: &Request<Incoming>, claims: &SessionClaims) -> bool {
    request
        .headers()
        .get(ADMIN_CSRF_HEADER)
        .and_then(|value| value.to_str().ok())
        .map(|value| constant_time_eq(value.as_bytes(), claims.csrf.as_bytes()))
        .unwrap_or(false)
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

fn with_set_cookie(response: &mut Response<Body>, cookie_value: String) {
    if let Ok(header) = HeaderValue::from_str(&cookie_header(cookie_value)) {
        response.headers_mut().append(header::SET_COOKIE, header);
    }
}

fn with_clear_cookie(response: &mut Response<Body>) {
    if let Ok(header) = HeaderValue::from_str(&expired_cookie()) {
        response.headers_mut().append(header::SET_COOKIE, header);
    }
}

async fn read_json<T: for<'de> serde::Deserialize<'de>>(
    request: Request<Incoming>,
) -> Result<T, Box<Response<Body>>> {
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
