//! Data types for the `ops` module.
#[allow(unused_imports)]
use super::*;
/// One operator action on the session fleet.
#[derive(Debug, Clone, PartialEq)]
pub enum SessionOp {
    /// Register a new session for `conversation`.
    Open {
        /// The conversation anchor — a peer cryptoId or an operator label.
        conversation: String,
        /// The lifetime class to open it in.
        class: SessionClass,
        /// The harness to serve it; `None` uses the manager's default.
        provider: Option<HarnessProvider>,
    },
    /// Run one turn on an open session.
    Submit {
        /// The session's local id.
        id: String,
        /// The prompt text.
        text: String,
    },
    /// Interrupt the turn in flight, leaving the session alive.
    Interrupt {
        /// The session's local id.
        id: String,
    },
    /// Drop the session's bound context so its next turn starts fresh.
    Reset {
        /// The session's local id.
        id: String,
    },
    /// Tear the session down.
    Close {
        /// The session's local id.
        id: String,
    },
    /// Drop a closed session's record and transcript.
    Forget {
        /// The session's local id.
        id: String,
    },
}
