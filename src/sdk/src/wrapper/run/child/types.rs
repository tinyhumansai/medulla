//! Data types for the `child` module.
#[allow(unused_imports)]
use super::*;
/// A spawned harness child, uniform across the PTY and inherited-stdio paths.
///
/// The run loop destructures this rather than calling methods on it, so each
/// field can be borrowed independently inside `tokio::select!`.
pub(in super::super) struct ChildSession {
    /// Sink for bytes typed into the child. `Some` only when input injection is
    /// active; [`drain_and_inject`](super::super::bridge::drain_and_inject)
    /// writes inbound owner messages here.
    pub(in super::super) input: Option<mpsc::UnboundedSender<Vec<u8>>>,
    /// Resolves with the shell-style exit code once the child has exited.
    pub(in super::super) done: oneshot::Receiver<i32>,
    /// Sending on this kills the child; consumed on first use.
    pub(in super::super) kill: Option<oneshot::Sender<()>>,
    /// Resolves once the PTY reader has copied the child's final output to our
    /// stdout. `None` on the inherited-stdio path, where the child wrote to the
    /// real stdout directly and nothing is buffered on our side.
    pub(in super::super) drained: Option<oneshot::Receiver<()>>,
    /// Returns the terminal to cooked mode (PTY path only). Called by the run
    /// loop once the child's output has drained.
    pub(in super::super) restore: Option<Box<dyn FnOnce() + Send>>,
}
