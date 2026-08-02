//! State owned by the live control-socket server.

use std::path::PathBuf;

use super::super::grants::{Grant, GrantRegistry};

/// What one control-socket connection has established about itself.
///
/// A connection starts unauthenticated and stays that way until a successful
/// `hello`; no other operation is answered before then.
#[derive(Default)]
pub struct SessionState {
    /// The redeemed grant and the token it came from, once `hello` succeeded.
    pub(super) authenticated: Option<(String, Grant)>,
}

impl SessionState {
    /// The grant this connection holds, if it has completed `hello`.
    pub fn grant(&self) -> Option<&Grant> {
        self.authenticated.as_ref().map(|(_, grant)| grant)
    }
}

/// A bound control socket, serving until dropped.
///
/// Dropping stops the accept loop, closes every connection already open, and
/// unlinks the socket file — the last only if the file at that path is still the
/// one this server bound, since a later drop must not delete the socket a
/// restarted instance has since put there.
pub struct ControlServer {
    pub(super) path: PathBuf,
    /// The socket identity at bind time, used to avoid unlinking a replacement.
    pub(super) identity: Option<(u64, u64)>,
    /// The listener task serving incoming control connections.
    pub(super) accept: tokio::task::JoinHandle<()>,
    /// Capabilities minted for harnesses spawned by this server.
    pub(super) grants: GrantRegistry,
    /// Flipped on drop; watched by the accept loop and every connection.
    pub(super) shutdown: tokio::sync::watch::Sender<bool>,
}
