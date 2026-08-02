//! The control socket listener.
//!
//! Byte plumbing only: frame a line, hand it to [`handle::handle_control`],
//! write the answer back. Every decision worth testing lives in that handler,
//! which is why this file has no branches a test would want to reach.

mod connection;
mod handle;
mod hub_ops;
#[cfg(test)]
mod hub_ops_tests;
mod registry;
#[cfg(test)]
mod registry_tests;
#[cfg(test)]
mod tests;
mod types;

pub use handle::{handle_control, MAX_WAIT};
pub use hub_ops::{FleetDefaults, HubFleetOps, HubSlot};
pub use types::{ControlServer, SessionState, TaskEntry, TaskRegistry, TaskState};

use std::path::Path;
use std::sync::Arc;

use super::grants::GrantRegistry;
use super::path::{prepare_bind, restrict_socket, ControlSocketError};
use super::types::FleetOps;
use connection::serve_connection;
#[cfg(test)]
use connection::{read_frame, MAX_FRAME_BYTES};

impl ControlServer {
    /// Bind `path` and start serving `ops`.
    ///
    /// `preserve_existing_parent` prevents Medulla from changing permissions on
    /// an existing directory named explicitly. Every path is still checked for
    /// insecure ancestors before bind; the default account-scoped directory is
    /// ours, so Medulla may tighten it first.
    ///
    /// # Errors
    ///
    /// Anything [`prepare_bind`] refuses — a live instance already holding the
    /// address, a non-socket file in the way, or a world-writable parent — plus
    /// the bind itself failing.
    #[cfg(unix)]
    pub async fn bind(
        path: &Path,
        ops: Arc<dyn FleetOps>,
        grants: GrantRegistry,
        preserve_existing_parent: bool,
    ) -> Result<Self, ControlSocketError> {
        // Held through `bind`: check+unlink without this claim lets two
        // starters both reclaim one stale inode and the second unlink the
        // first starter's freshly bound socket.
        let _bind_guard = prepare_bind(path, preserve_existing_parent).await?;
        let listener = tokio::net::UnixListener::bind(path)
            .map_err(|e| ControlSocketError::Io(e.to_string()))?;
        restrict_socket(path)?;
        let identity = socket_identity(path);

        let registry = TaskRegistry::new();
        let accept_grants = grants.clone();
        let (shutdown, _) = tokio::sync::watch::channel(false);
        let connection_shutdown = shutdown.clone();
        let mut accept_shutdown = shutdown.subscribe();
        let accept = tokio::spawn(async move {
            loop {
                let accepted = tokio::select! {
                    _ = accept_shutdown.changed() => return,
                    accepted = listener.accept() => accepted,
                };
                let Ok((stream, _peer)) = accepted else {
                    return;
                };
                let ops = ops.clone();
                let grants = accept_grants.clone();
                let registry = registry.clone();
                let shutdown = connection_shutdown.subscribe();
                tokio::spawn(async move {
                    let _ = serve_connection(stream, ops, grants, registry, shutdown).await;
                });
            }
        });

        Ok(ControlServer {
            path: path.to_path_buf(),
            identity,
            accept,
            grants,
            shutdown,
        })
    }

    /// The path this server is listening on.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// The grant registry this server redeems against.
    ///
    /// Handed to whatever spawns harnesses, so it can mint a grant for each one.
    pub fn grants(&self) -> &GrantRegistry {
        &self.grants
    }
}

/// The `(device, inode)` of the socket at `path`, when it can be read.
#[cfg(unix)]
fn socket_identity(path: &Path) -> Option<(u64, u64)> {
    use std::os::unix::fs::MetadataExt;

    std::fs::metadata(path)
        .ok()
        .map(|meta| (meta.dev(), meta.ino()))
}

impl Drop for ControlServer {
    fn drop(&mut self) {
        // Signalled before the accept task is aborted so open connections wind
        // themselves down too; aborting the acceptor alone would leave them
        // serving a fleet whose server is gone.
        self.shutdown.send_replace(true);
        self.accept.abort();
        #[cfg(unix)]
        {
            // Only our own socket. A restarted instance may already have bound
            // this path, and unlinking it would take the live control plane down
            // as a side effect of cleaning up a dead one.
            if socket_identity(&self.path) == self.identity {
                let _ = std::fs::remove_file(&self.path);
            }
        }
    }
}
