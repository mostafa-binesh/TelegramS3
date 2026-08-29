use crate::config::AppConfig;
use base64::{Engine as _, engine::general_purpose::STANDARD};
use std::fmt;
use std::sync::Arc;
use thiserror::Error;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::task::JoinHandle;
use url::Url;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProxyMode {
    Auto,
    Direct,
    Socks5,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProxyScheme {
    Socks5,
    Http,
    Https,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProxyTransportKind {
    Direct,
    Socks5,
    BridgedHttp,
    BridgedHttps,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProxyPlan {
    pub mode: ProxyMode,
    pub kind: ProxyTransportKind,
    pub proxy_url: Option<String>,
    pub upstream: Option<ProxyUpstream>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProxyUpstream {
    pub scheme: ProxyScheme,
    pub host: String,
    pub port: u16,
    pub username: Option<String>,
    pub password: Option<String>,
}

#[derive(Debug)]
pub struct ProxyBridgeHandle {
    pub local_port: u16,
    join_handle: JoinHandle<()>,
}

impl ProxyBridgeHandle {
    pub fn local_proxy_url(&self) -> String {
        format!("socks5://127.0.0.1:{}", self.local_port)
    }
}

impl Drop for ProxyBridgeHandle {
    fn drop(&mut self) {
        self.join_handle.abort();
    }
}

#[derive(Debug)]
pub struct MaterializedProxy {
    pub kind: ProxyTransportKind,
    pub proxy_url: Option<String>,
    pub bridge_active: bool,
    pub bridge: Option<ProxyBridgeHandle>,
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum ProxyError {
    #[error("proxy configuration is inconsistent: {0}")]
    Inconsistent(&'static str),
    #[error("proxy url is invalid: {0}")]
    InvalidUrl(String),
    #[error("proxy host is missing")]
    MissingHost,
    #[error("proxy port is invalid")]
    InvalidPort,
    #[error("unsupported proxy scheme: {0}")]
    UnsupportedScheme(String),
    #[error("bridge failed: {0}")]
    BridgeFailed(String),
}

pub fn resolve_proxy_plan(config: &AppConfig) -> Result<ProxyPlan, ProxyError> {
    let mode = match config.proxy_mode().to_ascii_lowercase().as_str() {
        "auto" => ProxyMode::Auto,
        "direct" => ProxyMode::Direct,
        "socks5" => ProxyMode::Socks5,
        other => return Err(ProxyError::UnsupportedScheme(other.to_string())),
    };

    let Some(raw_proxy_url) = config
        .telegram_proxy_url
        .as_deref()
        .filter(|value| !value.is_empty())
    else {
        return Ok(ProxyPlan {
            mode,
            kind: ProxyTransportKind::Direct,
            proxy_url: None,
            upstream: None,
        });
    };

    let parsed =
        Url::parse(raw_proxy_url).map_err(|err| ProxyError::InvalidUrl(err.to_string()))?;
    let upstream = proxy_upstream(&parsed, config)?;

    match mode {
        ProxyMode::Direct => Err(ProxyError::Inconsistent(
            "direct mode cannot be combined with TELEGRAM_PROXY_URL",
        )),
        ProxyMode::Socks5 => {
            if upstream.scheme != ProxyScheme::Socks5 {
                return Err(ProxyError::Inconsistent(
                    "socks5 mode requires a socks5:// proxy url",
                ));
            }
            Ok(ProxyPlan {
                mode,
                kind: ProxyTransportKind::Socks5,
                proxy_url: Some(render_socks5_url(&upstream)?),
                upstream: Some(upstream),
            })
        }
        ProxyMode::Auto => match upstream.scheme {
            ProxyScheme::Socks5 => Ok(ProxyPlan {
                mode,
                kind: ProxyTransportKind::Socks5,
                proxy_url: Some(render_socks5_url(&upstream)?),
                upstream: Some(upstream),
            }),
            ProxyScheme::Http => Ok(ProxyPlan {
                mode,
                kind: ProxyTransportKind::BridgedHttp,
                proxy_url: None,
                upstream: Some(upstream),
            }),
            ProxyScheme::Https => Ok(ProxyPlan {
                mode,
                kind: ProxyTransportKind::BridgedHttps,
                proxy_url: None,
                upstream: Some(upstream),
            }),
        },
    }
}

impl ProxyPlan {
    pub async fn materialize(&self) -> Result<MaterializedProxy, ProxyError> {
        match self.kind {
            ProxyTransportKind::Direct => Ok(MaterializedProxy {
                kind: self.kind,
                proxy_url: None,
                bridge_active: false,
                bridge: None,
            }),
            ProxyTransportKind::Socks5 => Ok(MaterializedProxy {
                kind: self.kind,
                proxy_url: self.proxy_url.clone(),
                bridge_active: false,
                bridge: None,
            }),
            ProxyTransportKind::BridgedHttp | ProxyTransportKind::BridgedHttps => {
                let upstream = self
                    .upstream
                    .clone()
                    .ok_or(ProxyError::Inconsistent("missing upstream bridge details"))?;
                let bridge = start_bridge(upstream).await?;
                Ok(MaterializedProxy {
                    kind: self.kind,
                    proxy_url: Some(bridge.local_proxy_url()),
                    bridge_active: true,
                    bridge: Some(bridge),
                })
            }
        }
    }
}

fn proxy_upstream(parsed: &Url, config: &AppConfig) -> Result<ProxyUpstream, ProxyError> {
    let scheme = match parsed.scheme() {
        "socks5" => ProxyScheme::Socks5,
        "http" => ProxyScheme::Http,
        "https" => ProxyScheme::Https,
        other => return Err(ProxyError::UnsupportedScheme(other.to_string())),
    };

    let host = parsed
        .host_str()
        .ok_or(ProxyError::MissingHost)?
        .to_string();
    let port = parsed
        .port_or_known_default()
        .ok_or(ProxyError::InvalidPort)?;

    let username = config
        .telegram_proxy_username
        .clone()
        .filter(|value| !value.is_empty())
        .or_else(|| {
            if parsed.username().is_empty() {
                None
            } else {
                Some(parsed.username().to_string())
            }
        });
    let password = config
        .telegram_proxy_password
        .clone()
        .filter(|value| !value.is_empty())
        .or_else(|| parsed.password().map(|value| value.to_string()));

    Ok(ProxyUpstream {
        scheme,
        host,
        port,
        username,
        password,
    })
}

fn render_socks5_url(upstream: &ProxyUpstream) -> Result<String, ProxyError> {
    let mut url = Url::parse("socks5://127.0.0.1:0")
        .map_err(|err| ProxyError::InvalidUrl(err.to_string()))?;
    url.set_host(Some(&upstream.host))
        .map_err(|_| ProxyError::InvalidUrl("invalid socks5 host".to_string()))?;
    url.set_port(Some(upstream.port))
        .map_err(|_| ProxyError::InvalidPort)?;
    if let Some(username) = &upstream.username {
        url.set_username(username)
            .map_err(|_| ProxyError::InvalidUrl("invalid proxy username".to_string()))?;
    }
    if let Some(password) = &upstream.password {
        url.set_password(Some(password))
            .map_err(|_| ProxyError::InvalidUrl("invalid proxy password".to_string()))?;
    }
    Ok(url.to_string())
}

async fn start_bridge(upstream: ProxyUpstream) -> Result<ProxyBridgeHandle, ProxyError> {
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .map_err(|error| ProxyError::BridgeFailed(error.to_string()))?;
    let local_port = listener
        .local_addr()
        .map_err(|error| ProxyError::BridgeFailed(error.to_string()))?
        .port();

    let join_handle = tokio::spawn(async move {
        while let Ok((client, _addr)) = listener.accept().await {
            let upstream = upstream.clone();
            tokio::spawn(async move {
                let _ = handle_bridge_client(client, upstream).await;
            });
        }
    });

    Ok(ProxyBridgeHandle {
        local_port,
        join_handle,
    })
}

async fn handle_bridge_client(
    mut client: TcpStream,
    upstream: ProxyUpstream,
) -> Result<(), ProxyError> {
    let mut greeting = [0u8; 2];
    client
        .read_exact(&mut greeting)
        .await
        .map_err(|error| ProxyError::BridgeFailed(error.to_string()))?;
    if greeting[0] != 0x05 {
        return Err(ProxyError::BridgeFailed(
            "unsupported SOCKS version".to_string(),
        ));
    }
    let mut methods = vec![0u8; greeting[1] as usize];
    client
        .read_exact(&mut methods)
        .await
        .map_err(|error| ProxyError::BridgeFailed(error.to_string()))?;
    client
        .write_all(&[0x05, 0x00])
        .await
        .map_err(|error| ProxyError::BridgeFailed(error.to_string()))?;

    let mut request = [0u8; 4];
    client
        .read_exact(&mut request)
        .await
        .map_err(|error| ProxyError::BridgeFailed(error.to_string()))?;
    if request[1] != 0x01 {
        return Err(ProxyError::BridgeFailed(
            "unsupported SOCKS command".to_string(),
        ));
    }

    let target_host = match request[3] {
        0x01 => {
            let mut ip = [0u8; 4];
            client
                .read_exact(&mut ip)
                .await
                .map_err(|error| ProxyError::BridgeFailed(error.to_string()))?;
            format!("{}.{}.{}.{}", ip[0], ip[1], ip[2], ip[3])
        }
        0x03 => {
            let mut len = [0u8; 1];
            client
                .read_exact(&mut len)
                .await
                .map_err(|error| ProxyError::BridgeFailed(error.to_string()))?;
            let mut bytes = vec![0u8; len[0] as usize];
            client
                .read_exact(&mut bytes)
                .await
                .map_err(|error| ProxyError::BridgeFailed(error.to_string()))?;
            String::from_utf8(bytes).map_err(|error| ProxyError::BridgeFailed(error.to_string()))?
        }
        0x04 => {
            let mut ip = [0u8; 16];
            client
                .read_exact(&mut ip)
                .await
                .map_err(|error| ProxyError::BridgeFailed(error.to_string()))?;
            format!(
                "[{:x}{:02x}:{:x}{:02x}:{:x}{:02x}:{:x}{:02x}:{:x}{:02x}:{:x}{:02x}:{:x}{:02x}:{:x}{:02x}]",
                ip[0],
                ip[1],
                ip[2],
                ip[3],
                ip[4],
                ip[5],
                ip[6],
                ip[7],
                ip[8],
                ip[9],
                ip[10],
                ip[11],
                ip[12],
                ip[13],
                ip[14],
                ip[15]
            )
        }
        _ => {
            return Err(ProxyError::BridgeFailed(
                "unsupported SOCKS address type".to_string(),
            ));
        }
    };

    let mut port = [0u8; 2];
    client
        .read_exact(&mut port)
        .await
        .map_err(|error| ProxyError::BridgeFailed(error.to_string()))?;
    let target_port = u16::from_be_bytes(port);

    let upstream_stream = TcpStream::connect((upstream.host.as_str(), upstream.port))
        .await
        .map_err(|error| ProxyError::BridgeFailed(error.to_string()))?;

    let mut upstream_stream: Box<dyn AsyncReadWrite> = match upstream.scheme {
        ProxyScheme::Https => {
            let mut root_store = rustls::RootCertStore::empty();
            root_store.extend(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
            let config = rustls::ClientConfig::builder()
                .with_root_certificates(root_store)
                .with_no_client_auth();
            let connector = tokio_rustls::TlsConnector::from(Arc::new(config));
            let server_name = rustls::pki_types::ServerName::try_from(upstream.host.clone())
                .map_err(|error| ProxyError::BridgeFailed(error.to_string()))?;
            let server_name = server_name.to_owned();
            let tls_stream = connector
                .connect(server_name, upstream_stream)
                .await
                .map_err(|error| ProxyError::BridgeFailed(error.to_string()))?;
            Box::new(tls_stream)
        }
        ProxyScheme::Http | ProxyScheme::Socks5 => Box::new(upstream_stream),
    };

    let mut connect_request = format!(
        "CONNECT {}:{} HTTP/1.1\r\nHost: {}:{}\r\nProxy-Connection: Keep-Alive\r\n",
        target_host, target_port, target_host, target_port
    );
    if let Some(username) = &upstream.username {
        let auth = format!(
            "{}:{}",
            username,
            upstream.password.as_deref().unwrap_or("")
        );
        connect_request.push_str(&format!(
            "Proxy-Authorization: Basic {}\r\n",
            STANDARD.encode(auth)
        ));
    }
    connect_request.push_str("\r\n");

    upstream_stream
        .write_all(connect_request.as_bytes())
        .await
        .map_err(|error| ProxyError::BridgeFailed(error.to_string()))?;
    upstream_stream
        .flush()
        .await
        .map_err(|error| ProxyError::BridgeFailed(error.to_string()))?;

    let mut response = Vec::new();
    let mut byte = [0u8; 1];
    loop {
        upstream_stream
            .read_exact(&mut byte)
            .await
            .map_err(|error| ProxyError::BridgeFailed(error.to_string()))?;
        response.push(byte[0]);
        if response.ends_with(b"\r\n\r\n") {
            break;
        }
        if response.len() > 8192 {
            return Err(ProxyError::BridgeFailed(
                "proxy CONNECT response headers too long".to_string(),
            ));
        }
    }

    let headers = String::from_utf8_lossy(&response);
    if !headers.starts_with("HTTP/1.1 200") && !headers.starts_with("HTTP/1.0 200") {
        return Err(ProxyError::BridgeFailed(format!(
            "proxy rejected CONNECT request: {}",
            headers.lines().next().unwrap_or("unknown status")
        )));
    }

    client
        .write_all(&[0x05, 0x00, 0x00, 0x01, 0, 0, 0, 0, 0, 0])
        .await
        .map_err(|error| ProxyError::BridgeFailed(error.to_string()))?;
    client
        .flush()
        .await
        .map_err(|error| ProxyError::BridgeFailed(error.to_string()))?;

    let (mut client_read, mut client_write) = tokio::io::split(client);
    let (mut upstream_read, mut upstream_write) = tokio::io::split(upstream_stream);
    let client_to_upstream = tokio::io::copy(&mut client_read, &mut upstream_write);
    let upstream_to_client = tokio::io::copy(&mut upstream_read, &mut client_write);
    tokio::select! {
        _ = client_to_upstream => {}
        _ = upstream_to_client => {}
    }
    Ok(())
}

trait AsyncReadWrite: AsyncRead + AsyncWrite + Unpin + Send {}
impl<T: AsyncRead + AsyncWrite + Unpin + Send> AsyncReadWrite for T {}

impl fmt::Display for ProxyPlan {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.kind {
            ProxyTransportKind::Direct => write!(f, "direct"),
            ProxyTransportKind::Socks5 => write!(
                f,
                "socks5:{}",
                self.proxy_url.as_deref().unwrap_or("<unset>")
            ),
            ProxyTransportKind::BridgedHttp => write!(f, "bridged-http"),
            ProxyTransportKind::BridgedHttps => write!(f, "bridged-https"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::AppConfig;

    #[test]
    fn resolves_direct_proxy_when_unset() {
        let plan = resolve_proxy_plan(&AppConfig::default()).expect("plan");
        assert_eq!(plan.kind, ProxyTransportKind::Direct);
        assert_eq!(plan.proxy_url, None);
    }

    #[test]
    fn rejects_direct_mode_with_proxy_url() {
        let config = AppConfig {
            telegram_proxy_url: Some("socks5://127.0.0.1:1080".to_string()),
            telegram_proxy_mode: Some("direct".to_string()),
            ..AppConfig::default()
        };
        assert!(matches!(
            resolve_proxy_plan(&config),
            Err(ProxyError::Inconsistent(_))
        ));
    }

    #[test]
    fn resolves_socks5_proxy_url() {
        let config = AppConfig {
            telegram_proxy_url: Some("socks5://127.0.0.1:1080".to_string()),
            telegram_proxy_mode: Some("socks5".to_string()),
            ..AppConfig::default()
        };
        let plan = resolve_proxy_plan(&config).expect("plan");
        assert_eq!(plan.kind, ProxyTransportKind::Socks5);
        assert!(
            plan.proxy_url
                .as_deref()
                .expect("proxy url")
                .starts_with("socks5://127.0.0.1:1080")
        );
    }

    #[test]
    fn resolves_http_proxy_to_bridge_plan() {
        let config = AppConfig {
            telegram_proxy_url: Some("http://proxy.example:8080".to_string()),
            telegram_proxy_mode: Some("auto".to_string()),
            ..AppConfig::default()
        };
        let plan = resolve_proxy_plan(&config).expect("plan");
        assert_eq!(plan.kind, ProxyTransportKind::BridgedHttp);
        assert!(plan.proxy_url.is_none());
    }
}
