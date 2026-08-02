//! State owned by the live control-socket server.

use std::path::PathBuf;

use super::super::grants::GrantRegistry;

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
