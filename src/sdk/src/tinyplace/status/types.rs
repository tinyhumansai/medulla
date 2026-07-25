//! Data types for the `status` module.
#[allow(unused_imports)]
use super::*;
/// The running state of the status machine.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionStatusState {
    /// `HarnessSessionState` wire string (see the `STATE_*` constants).
    pub state: String,
    pub detail: String,
    pub active_call_id: Option<String>,
    /// Timestamp of the last event that moved the machine (ms since epoch).
    pub last_event_at_ms: i64,
}
/// The result of a reduction/tick: the next state and, when something should be
/// published, the payload to emit.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StatusStep {
    pub next: SessionStatusState,
    pub emit: Option<StatusPayload>,
}
/// A semantic event fed to [`reduce_status`]: the typed event kind plus the
/// wall-clock time it occurred (ms since epoch). `None` or `0` means "unknown
/// time", which falls back to the machine's last activity clock.
#[derive(Debug, Clone, PartialEq)]
pub struct SemanticEvent {
    pub timestamp_ms: Option<i64>,
    pub event: HarnessEventKind,
}
pub(super) struct Derived {
    pub(super) state: String,
    pub(super) detail: String,
    pub(super) active_call_id: Option<String>,
}
