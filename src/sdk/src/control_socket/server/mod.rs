//! The control socket listener.
//!
//! Byte plumbing only: frame a line, hand it to [`handle::handle_control`],
//! write the answer back. Every decision worth testing lives in that handler,
//! which is why this file has no branches a test would want to reach.

mod handle;
mod hub_ops;
#[cfg(test)]
mod hub_ops_tests;
mod registry;

pub use handle::{handle_control, SessionState};
pub use hub_ops::{FleetDefaults, HubFleetOps, HubSlot};
pub use registry::{TaskEntry, TaskRegistry, TaskState};

use std::path::{Path, PathBuf};
use std::sync::Arc;

use serde_json::Value;

use super::grants::GrantRegistry;
use super::path::{prepare_bind, restrict_socket, ControlSocketError};
use super::types::FleetOps;

/// The largest frame the server will read.
///
/// An instruction can legitimately be long — it is a whole brief for an agent
/// with no context — so this is generous rather than tight. Its job is to stop
/// an endless line from consuming memory, not to police content.
const MAX_FRAME_BYTES: usize = 1024 * 1024;

/// How long a connection has to complete `hello`.
///
/// Bounded because an unauthenticated connection holding a slot forever is the
/// cheapest denial there is. Deliberately *not* applied afterwards: an
/// authenticated shim holds its connection open for the whole harness session
/// and is idle for most of it.
const HANDSHAKE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(5);

/// A bound control socket, serving until dropped.
///
/// Dropping stops the accept loop, closes every connection already open, and
/// unlinks the socket file — the last only if the file at that path is still the
/// one this server bound, since a later drop must not delete the socket a
/// restarted instance has since put there.
///
/// Closing live connections is the part worth stating: they hold their own
/// [`FleetOps`] handle, so a connection left running after its server was
/// dropped would keep dispatching into a fleet nothing is supervising any more.
pub struct ControlServer {
    path: PathBuf,
    /// The `(device, inode)` of the socket at bind time, so cleanup can tell
    /// "our socket" from "a newer instance's socket at the same path".
    identity: Option<(u64, u64)>,
    accept: tokio::task::JoinHandle<()>,
    grants: GrantRegistry,
    /// Flipped on drop; watched by the accept loop and every connection.
    shutdown: tokio::sync::watch::Sender<bool>,
}

impl ControlServer {
    /// Bind `path` and start serving `ops`.
    ///
    /// `trusted_path` relaxes the private-parent requirement for a path an
    /// operator named explicitly; the default account-scoped path is ours and is
    /// always checked.
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
        trusted_path: bool,
    ) -> Result<Self, ControlSocketError> {
        prepare_bind(path, trusted_path).await?;
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

/// Serve one connection until it closes.
#[cfg(unix)]
async fn serve_connection(
    stream: tokio::net::UnixStream,
    ops: Arc<dyn FleetOps>,
    grants: GrantRegistry,
    registry: TaskRegistry,
    mut shutdown: tokio::sync::watch::Receiver<bool>,
) -> std::io::Result<()> {
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

    let (read_half, mut write_half) = stream.into_split();
    let mut lines = BufReader::new(read_half).lines();
    let mut session = SessionState::default();

    loop {
        // The handshake is bounded; an authenticated connection is not, because
        // a shim legitimately sits idle between a harness's tool calls. Either
        // way the read races the server's shutdown, so dropping the server
        // closes this connection rather than leaving it parked on a read that
        // will never complete.
        let next = if session.grant().is_none() {
            tokio::select! {
                _ = shutdown.changed() => return Ok(()),
                read = tokio::time::timeout(HANDSHAKE_TIMEOUT, lines.next_line()) => match read {
                    Ok(next) => next?,
                    Err(_) => return Ok(()),
                },
            }
        } else {
            tokio::select! {
                _ = shutdown.changed() => return Ok(()),
                read = lines.next_line() => read?,
            }
        };
        let Some(line) = next else {
            return Ok(());
        };
        if line.trim().is_empty() {
            continue;
        }
        if line.len() > MAX_FRAME_BYTES {
            return Ok(());
        }
        let response = match serde_json::from_str::<Value>(&line) {
            Ok(request) => handle_control(&ops, &grants, &registry, &mut session, &request).await,
            Err(err) => serde_json::json!({
                "v": super::types::PROTOCOL_VERSION,
                "id": Value::Null,
                "ok": false,
                "error": {
                    "kind": super::types::ErrorKind::BadRequest.as_wire(),
                    "message": format!("could not parse frame: {err}"),
                    "retryable": false,
                },
            }),
        };
        write_half
            .write_all(format!("{response}\n").as_bytes())
            .await?;
        write_half.flush().await?;
    }
}
