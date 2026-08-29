pub mod proxy;
pub mod retry;
pub mod session;
pub mod transport;

pub use proxy::{
    MaterializedProxy, ProxyBridgeHandle, ProxyError, ProxyMode, ProxyPlan, ProxyScheme,
    ProxyTransportKind, resolve_proxy_plan,
};
pub use retry::{RetryDecision, RetryPolicy, parse_flood_wait_seconds};
pub use session::{SessionError, SessionStatus, TelegramSession};
pub use transport::{
    AuthFlowState, AuthState, LoginFailure, SessionState, TelegramTransport,
    TelegramTransportError, TelegramTransportStatus,
};
