//! The process-sharing seam: app-server connections keyed by identity, opened
//! once and reused by every task that can safely share them.
//!
//! This is where the efficiency the transport exists for actually happens.
//! Everything else in this module would work with a process per task; the pool
//! is what makes ten lanes cost one runtime.
//!
//! # Why a process-global pool
//!
//! Sharing has to outlive any one task, any one workflow run, and any one
//! session, or there is nothing to share. A pool owned by the daemon runtime
//! would already be too narrow — the TUI's local host and the daemon both
//! dispatch tasks in the same process and would each hold their own Codex.
//!
//! # Startup races
//!
//! Two lanes starting at once must not spawn two processes and discard one.
//! Acquisition therefore holds an async lock across the spawn, so the second
//! caller waits for the first one's handshake and gets the same connection. That
//! serialises *starting* a process, never using one.

use std::collections::HashMap;
use std::sync::{Arc, OnceLock};

use tokio::sync::Mutex;

use super::connection::Connection;
use super::types::{AppServerError, AppServerKey, AppServerSpec};

/// Long-lived `codex app-server` connections, keyed by process identity.
#[derive(Default)]
pub struct AppServerPool {
    connections: Mutex<HashMap<AppServerKey, Arc<Connection>>>,
}

impl AppServerPool {
    /// An empty pool.
    pub fn new() -> Self {
        Self::default()
    }

    /// The connection serving `spec`, spawning one if none is live.
    ///
    /// A cached connection whose child has died is discarded and replaced rather
    /// than returned. That check is on acquisition rather than a background
    /// health loop because it is the only moment the answer matters, and a
    /// reaper task would have to own restart policy for a process nobody is
    /// currently asking for.
    pub async fn acquire(&self, spec: &AppServerSpec) -> Result<Arc<Connection>, AppServerError> {
        let key = AppServerKey::from_spec(spec);
        let mut connections = self.connections.lock().await;
        if let Some(existing) = connections.get(&key) {
            if existing.is_alive() {
                return Ok(existing.clone());
            }
            tracing::info!(
                target: "medulla::codex_app_server",
                server = %key.label(),
                "replacing a dead codex app-server connection"
            );
            connections.remove(&key);
        }
        let connection = Connection::open(spec).await?;
        tracing::info!(
            target: "medulla::codex_app_server",
            server = %key.label(),
            "opened a codex app-server connection"
        );
        connections.insert(key, connection.clone());
        Ok(connection)
    }

    /// Drop every pooled connection, terminating the processes.
    ///
    /// For shutdown and for tests; ordinary operation never needs it, because a
    /// pooled process is the thing being conserved.
    pub async fn shutdown(&self) {
        self.connections.lock().await.clear();
    }

    /// How many connections are currently pooled.
    pub async fn len(&self) -> usize {
        self.connections.lock().await.len()
    }

    /// Whether the pool holds no connections.
    pub async fn is_empty(&self) -> bool {
        self.len().await == 0
    }
}

/// The process-wide pool.
///
/// See the module docs for why sharing is process-global rather than scoped to a
/// runtime: a narrower owner would spawn one Codex per owner and give most of
/// the saving back.
pub fn pool() -> &'static AppServerPool {
    static POOL: OnceLock<AppServerPool> = OnceLock::new();
    POOL.get_or_init(AppServerPool::new)
}
