use grammers_session::storages::MemorySession;
use grammers_session::types::{
    ChannelState, DcOption, PeerId, PeerInfo, UpdateState, UpdatesState,
};
use grammers_session::{Session, SessionData};
use std::collections::HashMap;
#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use thiserror::Error;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SessionStatus {
    Missing,
    Reusable,
    Reopened,
    Invalid,
    LoggedOut,
}

#[derive(Debug, Error)]
pub enum SessionError {
    #[error("failed to open Telegram session: {0}")]
    Open(String),
    #[error("failed to persist Telegram session: {0}")]
    Persist(String),
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
struct PersistedSessionState {
    home_dc: i32,
    dc_options: HashMap<i32, DcOption>,
    peer_infos: HashMap<PeerId, PeerInfo>,
    updates_state: UpdatesState,
}

impl Default for PersistedSessionState {
    fn default() -> Self {
        let data = SessionData::default();
        Self::from(data)
    }
}

impl From<SessionData> for PersistedSessionState {
    fn from(data: SessionData) -> Self {
        Self {
            home_dc: data.home_dc,
            dc_options: data.dc_options,
            peer_infos: data.peer_infos,
            updates_state: data.updates_state,
        }
    }
}

impl From<&PersistedSessionState> for SessionData {
    fn from(state: &PersistedSessionState) -> Self {
        Self {
            home_dc: state.home_dc,
            dc_options: state.dc_options.clone(),
            peer_infos: state.peer_infos.clone(),
            updates_state: state.updates_state.clone(),
        }
    }
}

pub struct TelegramSession {
    path: PathBuf,
    storage: Arc<PersistentSession>,
}

pub(crate) struct PersistentSession {
    inner: MemorySession,
    snapshot: Mutex<PersistedSessionState>,
    path: PathBuf,
}

impl TelegramSession {
    pub async fn open(path: impl AsRef<Path>) -> Result<Self, SessionError> {
        let path = path.as_ref().to_path_buf();
        if let Some(parent) = path.parent() {
            tokio::fs::create_dir_all(parent).await.ok();
        }
        create_private_session_file(&path).await?;
        let storage = PersistentSession::open(&path).await?;
        Ok(Self {
            path,
            storage: Arc::new(storage),
        })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub(crate) fn storage(&self) -> Arc<PersistentSession> {
        Arc::clone(&self.storage)
    }

    pub async fn reopen(path: impl AsRef<Path>) -> Result<Self, SessionError> {
        Self::open(path).await
    }

    pub fn status(&self) -> SessionStatus {
        if self.path.exists() {
            SessionStatus::Reusable
        } else {
            SessionStatus::Missing
        }
    }
}

impl PersistentSession {
    async fn open(path: &Path) -> Result<Self, SessionError> {
        let snapshot = load_snapshot(path).await?;
        let session_data = SessionData::from(&snapshot);
        let inner = MemorySession::from(session_data);
        Ok(Self {
            inner,
            snapshot: Mutex::new(snapshot),
            path: path.to_path_buf(),
        })
    }

    fn snapshot_mut(
        &self,
    ) -> Result<std::sync::MutexGuard<'_, PersistedSessionState>, SessionError> {
        self.snapshot
            .lock()
            .map_err(|_| SessionError::Persist("session lock is poisoned".to_string()))
    }

    async fn persist_snapshot(&self) -> Result<(), SessionError> {
        let snapshot = { self.snapshot_mut()?.clone() };
        let bytes = serde_json::to_vec_pretty(&snapshot)
            .map_err(|error| SessionError::Persist(error.to_string()))?;
        tokio::fs::write(&self.path, bytes)
            .await
            .map_err(|error| SessionError::Persist(error.to_string()))?;
        Ok(())
    }
}

impl Session for PersistentSession {
    type Error = SessionError;

    fn home_dc_id(&self) -> Result<i32, SessionError> {
        self.inner.home_dc_id().map_err(map_memory_error)
    }

    fn set_home_dc_id(
        &self,
        dc_id: i32,
    ) -> grammers_session::BoxFuture<'_, Result<(), SessionError>> {
        Box::pin(async move {
            self.inner
                .set_home_dc_id(dc_id)
                .await
                .map_err(map_memory_error)?;
            {
                let mut snapshot = self.snapshot_mut()?;
                snapshot.home_dc = dc_id;
            }
            self.persist_snapshot().await
        })
    }

    fn dc_option(&self, dc_id: i32) -> Result<Option<DcOption>, SessionError> {
        self.inner.dc_option(dc_id).map_err(map_memory_error)
    }

    fn set_dc_option(
        &self,
        dc_option: &DcOption,
    ) -> grammers_session::BoxFuture<'_, Result<(), SessionError>> {
        let dc_option = dc_option.clone();
        Box::pin(async move {
            self.inner
                .set_dc_option(&dc_option)
                .await
                .map_err(map_memory_error)?;
            {
                let mut snapshot = self.snapshot_mut()?;
                snapshot.dc_options.insert(dc_option.id, dc_option);
            }
            self.persist_snapshot().await
        })
    }

    fn peer(
        &self,
        peer: PeerId,
    ) -> grammers_session::BoxFuture<'_, Result<Option<PeerInfo>, SessionError>> {
        Box::pin(async move { self.inner.peer(peer).await.map_err(map_memory_error) })
    }

    fn cache_peer(
        &self,
        peer: &PeerInfo,
    ) -> grammers_session::BoxFuture<'_, Result<(), SessionError>> {
        let peer = peer.clone();
        Box::pin(async move {
            self.inner
                .cache_peer(&peer)
                .await
                .map_err(map_memory_error)?;
            {
                let mut snapshot = self.snapshot_mut()?;
                snapshot
                    .peer_infos
                    .entry(peer.id())
                    .or_insert_with(|| peer.clone())
                    .extend_info(&peer);
            }
            self.persist_snapshot().await
        })
    }

    fn updates_state(&self) -> grammers_session::BoxFuture<'_, Result<UpdatesState, SessionError>> {
        Box::pin(async move { self.inner.updates_state().await.map_err(map_memory_error) })
    }

    fn set_update_state(
        &self,
        update: UpdateState,
    ) -> grammers_session::BoxFuture<'_, Result<(), SessionError>> {
        Box::pin(async move {
            match update {
                UpdateState::All(updates_state) => {
                    self.inner
                        .set_update_state(UpdateState::All(updates_state.clone()))
                        .await
                        .map_err(map_memory_error)?;
                    let mut snapshot = self.snapshot_mut()?;
                    snapshot.updates_state = updates_state;
                }
                UpdateState::Primary { pts, date, seq } => {
                    self.inner
                        .set_update_state(UpdateState::Primary { pts, date, seq })
                        .await
                        .map_err(map_memory_error)?;
                    let mut snapshot = self.snapshot_mut()?;
                    snapshot.updates_state.pts = pts;
                    snapshot.updates_state.date = date;
                    snapshot.updates_state.seq = seq;
                }
                UpdateState::Secondary { qts } => {
                    self.inner
                        .set_update_state(UpdateState::Secondary { qts })
                        .await
                        .map_err(map_memory_error)?;
                    let mut snapshot = self.snapshot_mut()?;
                    snapshot.updates_state.qts = qts;
                }
                UpdateState::Channel { id, pts } => {
                    self.inner
                        .set_update_state(UpdateState::Channel { id, pts })
                        .await
                        .map_err(map_memory_error)?;
                    let mut snapshot = self.snapshot_mut()?;
                    snapshot
                        .updates_state
                        .channels
                        .retain(|channel| channel.id != id);
                    snapshot
                        .updates_state
                        .channels
                        .push(ChannelState { id, pts });
                }
            }
            self.persist_snapshot().await
        })
    }
}

async fn load_snapshot(path: &Path) -> Result<PersistedSessionState, SessionError> {
    let bytes = match tokio::fs::read(path).await {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Default::default()),
        Err(error) => return Err(SessionError::Open(error.to_string())),
    };

    if bytes.is_empty() {
        return Ok(Default::default());
    }

    serde_json::from_slice(&bytes).map_err(|error| SessionError::Open(error.to_string()))
}

fn map_memory_error(error: impl std::fmt::Display) -> SessionError {
    SessionError::Open(error.to_string())
}

async fn create_private_session_file(path: &Path) -> Result<(), SessionError> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        let _ = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .mode(0o600)
            .open(path)
            .map_err(|error| SessionError::Persist(error.to_string()))?;
        let mut permissions = std::fs::metadata(path)
            .map_err(|error| SessionError::Persist(error.to_string()))?
            .permissions();
        permissions.set_mode(0o600);
        std::fs::set_permissions(path, permissions)
            .map_err(|error| SessionError::Persist(error.to_string()))?;
    }
    #[cfg(not(unix))]
    {
        let _ = tokio::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
            .await
            .map_err(|error| SessionError::Persist(error.to_string()))?;
    }
    Ok(())
}
