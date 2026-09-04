use crate::config::{AppConfig, ConfigError};
use crate::telegram::proxy::{
    MaterializedProxy, ProxyError, ProxyTransportKind, resolve_proxy_plan,
};
use crate::telegram::retry::{RetryPolicy, parse_flood_wait_seconds};
use crate::telegram::session::{SessionError, SessionStatus, TelegramSession};
use grammers_client::Client;
use grammers_client::SignInError;
use grammers_mtsender::SenderPool;
use grammers_session::Session;
use grammers_session::types::{PeerId, PeerRef};
use std::io::{self, Write};
#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use tokio::sync::RwLock;
use thiserror::Error;
use tokio::task::JoinHandle;
use tokio::time::Duration;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SessionState {
    Missing,
    Reused,
    Unauthorized,
    Authorized,
    LoggedOut,
    Invalid,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AuthFlowState {
    Idle,
    WaitingForPhone,
    WaitingForCode { phone: String },
    WaitingForPassword { phone: String },
    Authorized,
    LoggedOut,
    Failed(LoginFailure),
}

impl AuthFlowState {
    pub fn waiting_for_phone() -> Self {
        Self::WaitingForPhone
    }

    pub fn waiting_for_code(phone: impl Into<String>) -> Self {
        Self::WaitingForCode {
            phone: phone.into(),
        }
    }

    pub fn waiting_for_password(phone: impl Into<String>) -> Self {
        Self::WaitingForPassword {
            phone: phone.into(),
        }
    }

    pub fn authorize(&mut self) {
        *self = Self::Authorized;
    }

    pub fn log_out(&mut self) {
        *self = Self::LoggedOut;
    }
}

impl SessionState {
    pub fn authorize(&mut self) {
        *self = Self::Authorized;
    }

    pub fn log_out(&mut self) {
        *self = Self::LoggedOut;
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LoginFailure {
    MissingCode,
    InvalidCode,
    ExpiredCode,
    WrongPassword,
    SignUpRequired,
    SessionInvalid,
    ProxyInvalid,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuthState {
    pub session: SessionState,
    pub flow: AuthFlowState,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TelegramTransportStatus {
    pub session_path: PathBuf,
    pub proxy_kind: ProxyTransportKind,
    pub proxy_url: Option<String>,
    pub session_state: SessionState,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TelegramConnectionState {
    Connected,
    Disconnected,
    NeedsReauth,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TelegramConnectionHealth {
    pub status: TelegramTransportStatus,
    pub state: TelegramConnectionState,
    pub detail: String,
}

#[derive(Debug, Error)]
pub enum TelegramTransportError {
    #[error("{0}")]
    Config(#[from] ConfigError),
    #[error("{0}")]
    Proxy(#[from] ProxyError),
    #[error("{0}")]
    Session(#[from] SessionError),
    #[error("Telegram RPC error: {0}")]
    Rpc(String),
    #[error("Telegram login failed: {0:?}")]
    Login(LoginFailure),
    #[error("io error: {0}")]
    Io(#[from] io::Error),
    #[error("invalid session state: {0}")]
    InvalidState(&'static str),
}

pub struct TelegramTransport {
    config: AppConfig,
    session_path: PathBuf,
    session: Option<TelegramSession>,
    client: Option<Client>,
    runner: Option<JoinHandle<()>>,
    retry_policy: RetryPolicy,
    proxy: MaterializedProxy,
    storage_peer_id: PeerId,
    storage_peer: Mutex<Option<PeerRef>>,
    mock_mode: bool,
}

#[derive(Clone)]
pub struct TelegramTransportManager {
    config: AppConfig,
    transport: std::sync::Arc<RwLock<std::sync::Arc<TelegramTransport>>>,
    health: std::sync::Arc<RwLock<TelegramConnectionHealth>>,
}

impl TelegramTransport {
    pub async fn open(config: AppConfig) -> Result<Self, TelegramTransportError> {
        config.validate()?;
        let mock_mode = matches!(
            std::env::var("TELEGRAM_TRANSPORT_RUNTIME"),
            Ok(value) if value.eq_ignore_ascii_case("mock")
        );

        let session_path = PathBuf::from(config.telegram_session_path.as_deref().ok_or(
            TelegramTransportError::InvalidState("missing session path after validation"),
        )?);

        let storage_chat_id =
            config
                .telegram_storage_chat_id
                .clone()
                .ok_or(TelegramTransportError::InvalidState(
                    "missing storage chat id after validation",
                ))?;
        let storage_peer_id = storage_chat_id
            .parse::<i64>()
            .map_err(|_| {
                TelegramTransportError::Config(ConfigError::Parse {
                    field: "TELEGRAM_STORAGE_CHAT_ID",
                    value: storage_chat_id.clone(),
                })
            })
            .and_then(|id| {
                PeerId::from_bot_api_dialog_id(id).ok_or_else(|| {
                    TelegramTransportError::Config(ConfigError::Parse {
                        field: "TELEGRAM_STORAGE_CHAT_ID",
                        value: storage_chat_id.clone(),
                    })
                })
            })?;

        let session = if mock_mode {
            if let Some(parent) = session_path.parent() {
                tokio::fs::create_dir_all(parent).await.ok();
            }
            create_private_mock_session(&session_path).await?;
            None
        } else {
            Some(TelegramSession::open(&session_path).await?)
        };

        let plan = resolve_proxy_plan(&config)?;
        let materialized = if mock_mode {
            MaterializedProxy {
                kind: plan.kind,
                proxy_url: plan.proxy_url.clone(),
                bridge_active: false,
                bridge: None,
            }
        } else {
            plan.materialize().await?
        };
        let api_id = config
            .telegram_api_id
            .as_deref()
            .ok_or(TelegramTransportError::InvalidState(
                "missing api id after validation",
            ))?
            .parse::<i32>()
            .map_err(|_| {
                TelegramTransportError::Config(ConfigError::Parse {
                    field: "TELEGRAM_API_ID",
                    value: config.telegram_api_id.clone().unwrap_or_default(),
                })
            })?;

        let connection_params = grammers_mtsender::ConnectionParams {
            proxy_url: materialized.proxy_url.clone(),
            ..Default::default()
        };
        let retry_policy = RetryPolicy::new(
            config.retry_count()?,
            Duration::from_millis(config.retry_backoff_ms()?),
            config.respect_flood_wait()?,
        );

        let (client, runner) = if mock_mode {
            (None, None)
        } else {
            let session_handle = session
                .as_ref()
                .ok_or(TelegramTransportError::InvalidState("missing live session"))?
                .storage();
            let pool = SenderPool::with_configuration(session_handle, api_id, connection_params);
            let SenderPool { handle, runner, .. } = pool;
            let client = Client::new(handle);
            let runner = tokio::spawn(async move {
                let _ = runner.run().await;
            });
            (Some(client), Some(runner))
        };

        let storage_peer = if mock_mode {
            Mutex::new(Some(storage_peer_id.to_ambient_ref()))
        } else {
            Mutex::new(None)
        };

        Ok(Self {
            config,
            session_path,
            session,
            client,
            runner,
            retry_policy,
            proxy: materialized,
            storage_peer_id,
            storage_peer,
            mock_mode,
        })
    }

    pub fn session_path(&self) -> &std::path::Path {
        &self.session_path
    }

    pub fn retry_policy(&self) -> RetryPolicy {
        self.retry_policy
    }

    pub fn proxy_kind(&self) -> ProxyTransportKind {
        self.proxy.kind
    }

    pub(crate) async fn storage_peer(&self) -> Result<PeerRef, TelegramTransportError> {
        if let Some(peer) = *self.storage_peer.lock().expect("storage peer mutex") {
            return Ok(peer);
        }

        if let Some(session) = self.session.as_ref()
            && let Ok(Some(peer)) = session.storage().peer_ref(self.storage_peer_id).await
        {
            *self.storage_peer.lock().expect("storage peer mutex") = Some(peer);
            return Ok(peer);
        }

        if self.mock_mode {
            let peer = self.storage_peer_id.to_ambient_ref();
            *self.storage_peer.lock().expect("storage peer mutex") = Some(peer);
            return Ok(peer);
        }

        let client = self.client()?;
        let mut dialogs = client.iter_dialogs();
        while let Some(dialog) = dialogs.next().await.map_err(map_rpc_error)? {
            if dialog.peer().id() == self.storage_peer_id {
                let peer = dialog.peer_ref();
                *self.storage_peer.lock().expect("storage peer mutex") = Some(peer);
                return Ok(peer);
            }
        }
        Err(TelegramTransportError::InvalidState(
            "storage peer dialog not found",
        ))
    }

    pub async fn auth_state(&self) -> Result<AuthState, TelegramTransportError> {
        if self.mock_mode {
            let local_state = if self.session_path.exists() {
                SessionState::Reused
            } else {
                SessionState::Missing
            };
            return Ok(AuthState {
                session: local_state.clone(),
                flow: if matches!(local_state, SessionState::Missing) {
                    AuthFlowState::Idle
                } else {
                    AuthFlowState::Authorized
                },
            });
        }

        let client = self.client()?;
        let authorized = client.is_authorized().await.map_err(map_rpc_error)?;
        Ok(AuthState {
            session: if authorized {
                SessionState::Authorized
            } else {
                SessionState::Unauthorized
            },
            flow: if authorized {
                AuthFlowState::Authorized
            } else {
                AuthFlowState::Idle
            },
        })
    }

    pub async fn status(&self) -> Result<TelegramTransportStatus, TelegramTransportError> {
        let state = self.auth_state().await?;
        let local_state = match self.session.as_ref() {
            Some(session) => match session.status() {
                SessionStatus::Reusable => SessionState::Reused,
                SessionStatus::Missing => SessionState::Missing,
                SessionStatus::LoggedOut => SessionState::LoggedOut,
                SessionStatus::Invalid => SessionState::Invalid,
                SessionStatus::Reopened => SessionState::Reused,
            },
            None => {
                if self.session_path.exists() {
                    SessionState::Reused
                } else {
                    SessionState::Missing
                }
            }
        };
        Ok(TelegramTransportStatus {
            session_path: self.session_path.clone(),
            proxy_kind: self.proxy.kind,
            proxy_url: self.proxy.proxy_url.clone(),
            session_state: if matches!(state.session, SessionState::Authorized) {
                SessionState::Authorized
            } else {
                local_state
            },
        })
    }

    pub async fn logout(&self) -> Result<SessionState, TelegramTransportError> {
        if self.mock_mode {
            return Ok(SessionState::LoggedOut);
        }

        let client = self.client()?;
        let _ = client.sign_out().await.map_err(map_rpc_error)?;
        Ok(SessionState::LoggedOut)
    }

    pub async fn bootstrap(&self) -> Result<TelegramTransportStatus, TelegramTransportError> {
        self.status().await
    }

    pub async fn interactive_login(&self) -> Result<AuthState, TelegramTransportError> {
        if self.mock_mode {
            let phone = prompt("phone number")?;
            if phone.trim().is_empty() {
                return Err(TelegramTransportError::Login(LoginFailure::MissingCode));
            }

            let code = prompt("authentication code")?;
            if code.trim().is_empty() {
                return Err(TelegramTransportError::Login(LoginFailure::MissingCode));
            }

            let mut flow = AuthFlowState::waiting_for_code(phone.trim().to_string());
            flow.authorize();
            return Ok(AuthState {
                session: SessionState::Authorized,
                flow,
            });
        }

        let client = self.client()?;
        if client.is_authorized().await.map_err(map_rpc_error)? {
            return Ok(AuthState {
                session: SessionState::Authorized,
                flow: AuthFlowState::Authorized,
            });
        }

        let phone = prompt("phone number")?;
        if phone.trim().is_empty() {
            return Err(TelegramTransportError::Login(LoginFailure::MissingCode));
        }
        let mut flow = AuthFlowState::waiting_for_code(phone.trim().to_string());

        let token = self
            .retry_rpc(|| client.request_login_code(phone.trim(), api_hash(&self.config)))
            .await?;

        let code = prompt("authentication code")?;
        if code.trim().is_empty() {
            return Err(TelegramTransportError::Login(LoginFailure::MissingCode));
        }

        match client.sign_in(&token, code.trim()).await {
            Ok(_) => Ok(AuthState {
                session: SessionState::Authorized,
                flow: {
                    flow.authorize();
                    flow
                },
            }),
            Err(SignInError::PasswordRequired(password_token)) => {
                let password = prompt("2FA password")?;
                if password.trim().is_empty() {
                    return Err(TelegramTransportError::Login(LoginFailure::WrongPassword));
                }
                flow = AuthFlowState::waiting_for_password(phone.trim().to_string());
                client
                    .check_password(password_token, password.trim())
                    .await
                    .map_err(|error| {
                        TelegramTransportError::Login(login_failure_from_sign_in(error))
                    })?;
                Ok(AuthState {
                    session: SessionState::Authorized,
                    flow: {
                        flow.authorize();
                        flow
                    },
                })
            }
            Err(error) => Err(TelegramTransportError::Login(login_failure_from_sign_in(
                error,
            ))),
        }
    }

    async fn retry_rpc<T, F, Fut>(&self, mut op: F) -> Result<T, TelegramTransportError>
    where
        F: FnMut() -> Fut,
        Fut: std::future::Future<Output = Result<T, grammers_client::InvocationError>>,
    {
        let mut attempt = 1;
        loop {
            let result = op().await;
            match result {
                Ok(value) => return Ok(value),
                Err(error) => {
                    let flood_wait = parse_flood_wait_seconds(error.to_string());
                    match self.retry_policy.retry_decision(attempt, flood_wait) {
                        crate::telegram::retry::RetryDecision::RetryAfter(delay) => {
                            tokio::time::sleep(delay).await;
                            attempt += 1;
                        }
                        crate::telegram::retry::RetryDecision::RespectFloodWait(delay) => {
                            tokio::time::sleep(delay).await;
                            attempt += 1;
                        }
                        crate::telegram::retry::RetryDecision::GiveUp => {
                            return Err(TelegramTransportError::Rpc(error.to_string()));
                        }
                    }
                }
            }
        }
    }
}

impl TelegramTransportManager {
    pub async fn open(config: AppConfig) -> Result<std::sync::Arc<Self>, TelegramTransportError> {
        let transport = std::sync::Arc::new(TelegramTransport::open(config.clone()).await?);
        let health = evaluate_health(transport.as_ref()).await?;
        Ok(std::sync::Arc::new(Self {
            config,
            transport: std::sync::Arc::new(RwLock::new(transport)),
            health: std::sync::Arc::new(RwLock::new(health)),
        }))
    }

    pub async fn current(&self) -> std::sync::Arc<TelegramTransport> {
        self.transport.read().await.clone()
    }

    pub async fn health(&self) -> TelegramConnectionHealth {
        self.health.read().await.clone()
    }

    pub async fn refresh(&self) -> Result<TelegramConnectionHealth, TelegramTransportError> {
        let transport = std::sync::Arc::new(TelegramTransport::open(self.config.clone()).await?);
        let health = evaluate_health(transport.as_ref()).await?;
        *self.transport.write().await = transport;
        *self.health.write().await = health.clone();
        Ok(health)
    }
}

async fn create_private_mock_session(path: &Path) -> Result<(), io::Error> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        let _ = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .mode(0o600)
            .open(path)?;
        let mut permissions = std::fs::metadata(path)?.permissions();
        permissions.set_mode(0o600);
        std::fs::set_permissions(path, permissions)?;
    }
    #[cfg(not(unix))]
    {
        let _ = tokio::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
            .await?;
    }
    Ok(())
}

impl Drop for TelegramTransport {
    fn drop(&mut self) {
        if let Some(runner) = &self.runner {
            runner.abort();
        }
    }
}

fn api_hash(config: &AppConfig) -> &str {
    config.telegram_api_hash.as_deref().unwrap_or_default()
}

fn prompt(label: &str) -> Result<String, TelegramTransportError> {
    print!("{label}: ");
    io::stdout().flush()?;
    let mut value = String::new();
    io::stdin().read_line(&mut value)?;
    Ok(value.trim_end_matches(['\r', '\n']).to_string())
}

fn map_rpc_error(error: impl std::fmt::Display) -> TelegramTransportError {
    TelegramTransportError::Rpc(error.to_string())
}

impl TelegramTransport {
    pub(crate) fn client(&self) -> Result<&Client, TelegramTransportError> {
        self.client
            .as_ref()
            .ok_or(TelegramTransportError::InvalidState(
                "real Telegram client is unavailable",
            ))
    }
}

async fn evaluate_health(
    transport: &TelegramTransport,
) -> Result<TelegramConnectionHealth, TelegramTransportError> {
    let status = transport.status().await?;
    let (state, detail) = match status.session_state {
        SessionState::Authorized | SessionState::Reused => match transport.storage_peer().await {
            Ok(_) => (
                TelegramConnectionState::Connected,
                "storage chat reachable".to_string(),
            ),
            Err(error) => (
                TelegramConnectionState::Disconnected,
                format!("storage peer lookup failed: {error}"),
            ),
        },
        SessionState::Missing | SessionState::Unauthorized | SessionState::Invalid => (
            TelegramConnectionState::NeedsReauth,
            "telegram session needs reauthorization".to_string(),
        ),
        SessionState::LoggedOut => (
            TelegramConnectionState::Disconnected,
            "telegram session is logged out".to_string(),
        ),
    };
    Ok(TelegramConnectionHealth {
        status,
        state,
        detail,
    })
}

impl TelegramTransport {
    /// Whether the transport is running against the local mock runtime. Used to
    /// drive scripted login flows in tests and CI.
    pub(crate) fn is_mock(&self) -> bool {
        self.mock_mode
    }

    /// Borrow the live grammers client, if this transport holds one.
    pub(crate) fn login_client(&self) -> Result<&Client, TelegramTransportError> {
        self.client()
    }

    pub(crate) fn app_api_hash(&self) -> &str {
        self.config.telegram_api_hash.as_deref().unwrap_or_default()
    }

    /// Run an RPC operation under the shared retry / flood-wait policy so the
    /// HTTP wizard and the CLI login behave identically on transient failures.
    pub(crate) async fn retry_invocation<T, F, Fut>(
        &self,
        op: F,
    ) -> Result<T, TelegramTransportError>
    where
        F: FnMut() -> Fut,
        Fut: std::future::Future<Output = Result<T, grammers_client::InvocationError>>,
    {
        self.retry_rpc(op).await
    }
}

fn login_failure_from_sign_in(error: SignInError) -> LoginFailure {
    login_failure_from_text(&error.to_string())
}

pub(crate) fn login_failure_from_text(text: &str) -> LoginFailure {
    if text.contains("PHONE_CODE_EXPIRED") {
        LoginFailure::ExpiredCode
    } else if text.contains("PHONE_CODE_EMPTY") {
        LoginFailure::MissingCode
    } else if text.contains("PHONE_CODE_INVALID") {
        LoginFailure::InvalidCode
    } else if text.contains("PASSWORD")
        || text.contains("2FA")
        || text.contains("SESSION_PASSWORD_NEEDED")
    {
        LoginFailure::WrongPassword
    } else if text.contains("SIGN_UP_REQUIRED") {
        LoginFailure::SignUpRequired
    } else {
        LoginFailure::SessionInvalid
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::telegram::retry::RetryPolicy;
    use std::time::Duration;

    #[test]
    fn login_flow_transitions_are_explicit() {
        let mut flow = AuthFlowState::waiting_for_phone();
        assert!(matches!(flow, AuthFlowState::WaitingForPhone));

        flow = AuthFlowState::waiting_for_code("+15551234567");
        assert!(matches!(
            flow,
            AuthFlowState::WaitingForCode { phone } if phone == "+15551234567"
        ));

        flow = AuthFlowState::waiting_for_password("+15551234567");
        assert!(matches!(
            flow,
            AuthFlowState::WaitingForPassword { ref phone } if phone == "+15551234567"
        ));

        flow.authorize();
        assert_eq!(flow, AuthFlowState::Authorized);

        flow.log_out();
        assert_eq!(flow, AuthFlowState::LoggedOut);
    }

    #[test]
    fn login_failure_has_distinct_variants() {
        assert_eq!(LoginFailure::MissingCode, LoginFailure::MissingCode);
        assert_eq!(LoginFailure::InvalidCode, LoginFailure::InvalidCode);
        assert_eq!(LoginFailure::ExpiredCode, LoginFailure::ExpiredCode);
        assert_eq!(LoginFailure::WrongPassword, LoginFailure::WrongPassword);
    }

    #[test]
    fn login_failure_classification_covers_common_server_errors() {
        assert_eq!(
            login_failure_from_text("PHONE_CODE_EMPTY"),
            LoginFailure::MissingCode
        );
        assert_eq!(
            login_failure_from_text("PHONE_CODE_EXPIRED"),
            LoginFailure::ExpiredCode
        );
        assert_eq!(
            login_failure_from_text("SESSION_PASSWORD_NEEDED"),
            LoginFailure::WrongPassword
        );
    }

    #[test]
    fn session_state_has_clear_variants() {
        assert_eq!(SessionState::Missing, SessionState::Missing);
        assert_eq!(SessionState::LoggedOut, SessionState::LoggedOut);
    }

    #[test]
    fn retry_policy_is_usable_from_transport() {
        let policy = RetryPolicy::new(3, Duration::from_millis(500), true);
        assert_eq!(policy.max_attempts, 3);
    }
}
