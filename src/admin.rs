//! Authenticated operator/admin HTTP surface for the `/_admin` SPA.
//!
//! Serves the (self-hosted, hidden-behind-login) management console on the same
//! public listener as the S3 data plane. Authentication is credential based:
//! accounts live in `metadata::users` (argon2id), sessions are signed cookies
//! bound to a `admin_sessions` row so they can be individually revoked and are
//! invalidated globally on password change / user disable via `token_version`.
//!
//! Phase-9 core scope: login / logout / refresh / whoami, user CRUD, and the
//! JSON file-management surface (buckets + prefix/folder listing + zero-byte
//! directory markers + tombstones). Binary content streaming (upload/download)
//! and the in-browser Telegram onboarding wizard are designed next and are not
//! yet wired here (see ADR-0006 / ROADMAP).

use crate::auth::{self, AuthError, LoginLimiter};
use crate::config::AppConfig;
use crate::manifest::ObjectManifest;
use crate::metadata::MetadataStore;
use crate::object_format::ObjectFormatService;
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
const ADMIN_SESSION_TTL_SECONDS: i64 = 8 * 60 * 60;
const ADMIN_CSRF_HEADER: &str = "x-csrf-token";

#[derive(Clone)]
pub struct AdminUiState {
    config: AppConfig,
    object_format: Arc<ObjectFormatService>,
    transport_status: TelegramTransportStatus,
    s3_addr: SocketAddr,
    admin_addr: SocketAddr,
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
struct DeleteObjectRequest {
    bucket: String,
    key: String,
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

fn rfc3339(value: OffsetDateTime) -> String {
    value
        .format(&time::format_description::well_known::Rfc3339)
        .unwrap_or_else(|_| value.unix_timestamp().to_string())
}

impl AdminUiState {
    pub fn new(
        config: AppConfig,
        object_format: Arc<ObjectFormatService>,
        transport_status: TelegramTransportStatus,
        s3_addr: SocketAddr,
        admin_addr: SocketAddr,
        cookie_secret: String,
        ui_dist_dir: PathBuf,
    ) -> Self {
        Self {
            config,
            object_format,
            transport_status,
            s3_addr,
            admin_addr,
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

        match (method, rest) {
            (Method::POST, "session/logout") => self.handle_logout(&principal).await,
            (Method::POST, "session/refresh") => self.handle_refresh(&principal).await,
            (Method::GET, "overview") => self.handle_overview(&principal),
            (Method::GET, "users") => self.handle_list_users(),
            (Method::POST, "users") => self.handle_create_user(request, &principal).await,
            (Method::POST, p) if p.starts_with("users/") => {
                self.handle_change_password(request, p, &principal).await
            }
            (Method::DELETE, p) if p.starts_with("users/") => {
                self.handle_delete_user(p, &principal)
            }
            (Method::GET, "buckets") => self.handle_list_buckets(),
            (Method::GET, "objects") => self.handle_list_objects(request),
            (Method::POST, "objects/folder") => {
                self.handle_create_folder(request, &principal).await
            }
            (Method::POST, "objects/delete") => {
                self.handle_delete_object(request, &principal).await
            }
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
        if auth::is_superadmin(&target) && self.store().user_count().unwrap_or(0) <= 1 {
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
            objects.push(object_to_wire(&manifest, key));
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
        let manifest =
            match self
                .object_format
                .put_bytes(&bucket, &path, "application/directory", &[])
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
        match self.object_format.delete_object(&bucket, &key) {
            Ok(_) => json_response(StatusCode::OK, serde_json::json!({ "ok": true })),
            Err(error) => json_error(StatusCode::INTERNAL_SERVER_ERROR, &error.to_string()),
        }
    }

    fn handle_overview(&self, principal: &ResolvedPrincipal) -> Response<Body> {
        let metadata_status = self
            .object_format
            .metadata_status()
            .unwrap_or_else(|_| empty_meta());
        let object_status = self
            .object_format
            .status()
            .unwrap_or_else(|_| empty_object());
        let state_ref = &self.transport_status.session_state;
        let session_state_debug = format!("{state_ref:?}");
        let session_usable = telegram_session_usable(state_ref.clone());
        let telegram = TelegramStateWire {
            session_state: session_state_debug.clone(),
            phone_number: self
                .config
                .telegram_phone_number
                .as_deref()
                .map(redact_phone_number),
        };
        let endpoint = EndpointWire {
            s3_bind_addr: self.s3_addr.to_string(),
            admin_bind_addr: self.admin_addr.to_string(),
            admin_route_prefix: ADMIN_ROUTE_PREFIX.to_string(),
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
            check("Telegram auth", session_usable, &session_state_debug),
            check(
                "Storage chat",
                self.config
                    .telegram_storage_chat_id
                    .as_deref()
                    .is_some_and(|v| !v.is_empty()),
                "configured",
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
                "checked_at": rfc3339(OffsetDateTime::now_utc()),
                "session": {"authenticated": true, "user": UserWire::from_user(&principal.user)},
                "endpoint": endpoint,
                "storage": storage,
                "telegram": telegram,
                "checks": checks,
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
            let Some(file_path) = safe_join(self.ui_dist_dir(), asset_path) else {
                return json_error(StatusCode::NOT_FOUND, "asset not found");
            };
            return match read_static_file(&file_path).await {
                Ok(Some(response)) => response,
                Ok(None) => asset_fallback(self.ui_dist_dir(), asset_path)
                    .await
                    .unwrap_or_else(|| json_error(StatusCode::NOT_FOUND, "asset not found")),
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

async fn asset_fallback(base: &Path, requested_asset: &str) -> Option<Response<Body>> {
    let extension = Path::new(requested_asset)
        .extension()
        .and_then(|value| value.to_str())
        .unwrap_or_default();
    if extension.is_empty() {
        return None;
    }
    let assets_dir = base.join("assets");
    let mut directory = fs::read_dir(assets_dir).await.ok()?;
    while let Some(entry) = directory.next_entry().await.ok()? {
        let path = entry.path();
        if path
            .extension()
            .and_then(|value| value.to_str())
            .is_some_and(|ext| ext == extension)
        {
            return read_static_file(&path).await.ok().flatten();
        }
    }
    None
}

// ---- helpers used by the admin implementation -------------------------------

fn telegram_session_usable(state: SessionState) -> bool {
    matches!(state, SessionState::Authorized | SessionState::Reused)
}

fn object_to_wire(manifest: &ObjectManifest, key: &str) -> ObjectEntryWire {
    ObjectEntryWire {
        name: basename_key(key),
        key: key.to_string(),
        size: manifest.content_length,
        last_modified: rfc3339(manifest.created_at),
        etag: manifest.checksum.whole_object.clone(),
    }
}

fn basename_key(key: &str) -> String {
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
struct EndpointWire {
    s3_bind_addr: String,
    admin_bind_addr: String,
    admin_route_prefix: String,
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
struct TelegramStateWire {
    session_state: String,
    phone_number: Option<String>,
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
