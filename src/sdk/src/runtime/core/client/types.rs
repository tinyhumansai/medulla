//! Data types for the `client` module.
#[allow(unused_imports)]
use super::*;
/// A [`Runtime`](crate::runtime::Runtime) attached to a `medulla-serve` process
/// over a unix domain socket. Snapshot/subscribe read the shared state the
/// driver folds into; the mutating methods enqueue [`Command`]s the driver
/// serializes onto the wire.
pub struct CoreRuntime {
    /// Shared connection state the snapshot is rendered from.
    pub(in super::super) state: Arc<Mutex<CoreState>>,
    /// Pinged after every fold/mutation so the UI re-pulls a snapshot.
    pub(in super::super) tx: broadcast::Sender<()>,
    /// Commands to the connection driver.
    pub(in super::super) cmd_tx: mpsc::UnboundedSender<Command>,
    /// The attached socket path, for diagnostics.
    pub(in super::super) socket_path: PathBuf,
    /// The capacity and template catalog this client declared at handshake
    /// time, mirrored onto every snapshot so the fleet surfaces can render it.
    pub(in super::super) declarations: Arc<CoreDeclarations>,
    /// The driver task handle, aborted on drop.
    pub(super) driver: Mutex<Option<JoinHandle<()>>>,
}
/// How one connection attempt ended, deciding whether the driver re-attaches.
pub(super) enum ConnOutcome {
    /// The command channel closed (handle dropped) or a clean `shutdown`.
    Shutdown,
    /// A fatal handshake outcome; carries the reason. The driver stops retrying.
    Fatal(String),
    /// The socket dropped mid-session (or was not yet present). Re-attach.
    Dropped,
}
/// The serve identity learned from a successful handshake.
pub(super) struct HelloOk {
    /// The serve build version (from `ready`).
    pub(super) serve: Option<String>,
    /// The session id serve owns (from `ready`).
    pub(super) session_id: Option<String>,
}
/// How the handshake can fail.
pub(super) enum HandshakeError {
    /// A permanent rejection (version mismatch / `hello` error / startup error).
    Fatal(String),
    /// A transport drop mid-handshake; the driver re-attaches.
    Dropped,
}
