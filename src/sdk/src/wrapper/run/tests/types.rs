//! Test-only data types for wrapper child spawning.

use tokio::sync::{mpsc, oneshot};

use crate::wrapper::PtyRequest;

/// The receiving ends of a stub PTY harness, kept alive by the test.
pub(super) struct StubPty {
    pub(super) input: mpsc::UnboundedReceiver<Vec<u8>>,
    pub(super) done: oneshot::Sender<i32>,
    pub(super) kill: oneshot::Receiver<()>,
    pub(super) requested: std::sync::Arc<std::sync::Mutex<Option<PtyRequest>>>,
}
