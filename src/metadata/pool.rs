use super::MetadataError;
use rusqlite::{Connection, OpenFlags};
use std::path::{Path, PathBuf};
use std::sync::{Condvar, Mutex};
use std::time::Duration;

pub(crate) const DEFAULT_CONNECTION_POOL_SIZE: usize = 8;
const SQLITE_BUSY_TIMEOUT: Duration = Duration::from_secs(30);

#[derive(Debug, Clone)]
enum PoolSource {
    File(PathBuf),
    SharedMemory(String),
}

#[derive(Debug)]
struct PoolState {
    available: Vec<Connection>,
    open_connections: usize,
}

/// A small, bounded pool of SQLite handles.
///
/// SQLite still serializes writers, but WAL allows readers to proceed while a
/// writer is active. Keeping independent handles prevents one poisoned or
/// interrupted handle from becoming a process-wide lock, while the busy
/// timeout keeps normal writer contention recoverable.
pub(crate) struct ConnectionPool {
    source: PoolSource,
    max_size: usize,
    state: Mutex<PoolState>,
    available: Condvar,
}

impl ConnectionPool {
    pub(crate) fn open_file(path: impl AsRef<Path>, size: usize) -> Result<Self, MetadataError> {
        Self::new(PoolSource::File(path.as_ref().to_path_buf()), size)
    }

    pub(crate) fn open_shared_memory(
        name: impl Into<String>,
        size: usize,
    ) -> Result<Self, MetadataError> {
        Self::new(PoolSource::SharedMemory(name.into()), size)
    }

    fn new(source: PoolSource, size: usize) -> Result<Self, MetadataError> {
        let max_size = size.max(1);
        let mut connections = Vec::with_capacity(max_size);
        for _ in 0..max_size {
            connections.push(open_connection(&source)?);
        }
        Ok(Self {
            source,
            max_size,
            state: Mutex::new(PoolState {
                available: connections,
                open_connections: max_size,
            }),
            available: Condvar::new(),
        })
    }

    pub(crate) fn checkout(&self) -> Result<PooledConnection<'_>, MetadataError> {
        let mut state = self.state.lock().map_err(|_| MetadataError::Poisoned)?;
        loop {
            if let Some(connection) = state.available.pop() {
                return Ok(PooledConnection {
                    pool: self,
                    connection: Some(connection),
                    discard: false,
                });
            }

            if state.open_connections < self.max_size {
                state.open_connections += 1;
                drop(state);
                match open_connection(&self.source) {
                    Ok(connection) => {
                        return Ok(PooledConnection {
                            pool: self,
                            connection: Some(connection),
                            discard: false,
                        });
                    }
                    Err(error) => {
                        let mut state = self.state.lock().map_err(|_| MetadataError::Poisoned)?;
                        state.open_connections = state.open_connections.saturating_sub(1);
                        self.available.notify_one();
                        return Err(error);
                    }
                }
            }

            state = self
                .available
                .wait(state)
                .map_err(|_| MetadataError::Poisoned)?;
        }
    }

    fn return_connection(&self, connection: Option<Connection>) {
        let replace = {
            let Ok(mut state) = self.state.lock() else {
                return;
            };
            if let Some(connection) = connection {
                state.available.push(connection);
                false
            } else {
                state.open_connections = state.open_connections.saturating_sub(1);
                if state.open_connections < self.max_size {
                    state.open_connections += 1;
                    true
                } else {
                    false
                }
            }
        };

        if replace {
            match open_connection(&self.source) {
                Ok(connection) => {
                    if let Ok(mut state) = self.state.lock() {
                        state.available.push(connection);
                    }
                }
                Err(error) => {
                    if let Ok(mut state) = self.state.lock() {
                        state.open_connections = state.open_connections.saturating_sub(1);
                    }
                    tracing::warn!(error = %error, "metadata connection pool replacement failed");
                }
            }
        }
        self.available.notify_one();
    }

    #[cfg(test)]
    fn open_connection_count(&self) -> usize {
        self.state.lock().expect("pool state").open_connections
    }
}

pub(crate) struct PooledConnection<'a> {
    pool: &'a ConnectionPool,
    connection: Option<Connection>,
    discard: bool,
}

impl PooledConnection<'_> {
    pub(crate) fn as_mut(&mut self) -> &mut Connection {
        self.connection.as_mut().expect("pooled connection present")
    }

    pub(crate) fn discard(&mut self) {
        self.discard = true;
    }
}

impl Drop for PooledConnection<'_> {
    fn drop(&mut self) {
        let connection = if self.discard {
            None
        } else {
            self.connection.take()
        };
        self.pool.return_connection(connection);
    }
}

fn open_connection(source: &PoolSource) -> Result<Connection, MetadataError> {
    let connection = match source {
        PoolSource::File(path) => Connection::open(path)?,
        PoolSource::SharedMemory(name) => Connection::open_with_flags(
            name,
            OpenFlags::SQLITE_OPEN_READ_WRITE
                | OpenFlags::SQLITE_OPEN_CREATE
                | OpenFlags::SQLITE_OPEN_URI,
        )?,
    };
    connection.busy_timeout(SQLITE_BUSY_TIMEOUT)?;
    connection.pragma_update(None, "foreign_keys", true)?;
    if matches!(source, PoolSource::File(_)) {
        connection.pragma_update(None, "journal_mode", "WAL")?;
    }
    connection.pragma_update(None, "synchronous", "FULL")?;
    Ok(connection)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn discarding_a_handle_allows_the_pool_to_replace_it() {
        let pool = ConnectionPool::open_shared_memory(
            "file:telegram_s3_pool_test?mode=memory&cache=shared",
            2,
        )
        .expect("pool");
        assert_eq!(pool.open_connection_count(), 2);

        let mut connection = pool.checkout().expect("checkout");
        connection.discard();
        drop(connection);
        assert_eq!(pool.open_connection_count(), 2);

        let replacement = pool.checkout().expect("replacement");
        assert_eq!(pool.open_connection_count(), 2);
        drop(replacement);
    }

    #[test]
    fn independent_handles_support_parallel_reads() {
        let pool = ConnectionPool::open_shared_memory(
            "file:telegram_s3_parallel_pool_test?mode=memory&cache=shared",
            4,
        )
        .expect("pool");

        std::thread::scope(|scope| {
            let handles = (0..16)
                .map(|_| {
                    scope.spawn(|| {
                        let mut connection = pool.checkout().expect("checkout");
                        connection
                            .as_mut()
                            .query_row("SELECT 1", [], |row| row.get::<_, i64>(0))
                            .expect("read")
                    })
                })
                .collect::<Vec<_>>();
            for handle in handles {
                assert_eq!(handle.join().expect("worker"), 1);
            }
        });
    }
}
