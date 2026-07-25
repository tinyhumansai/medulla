//! Data types for the `bridge` module.
#[allow(unused_imports)]
use super::*;
/// The tiny.place bridge for one wrapped session: encrypted transport plus the
/// per-session envelope/status/tailer state. Absent when running passthrough.
pub(in super::super) struct Bridge {
    pub(in super::super) transport: SignalTransport,
    pub(in super::super) recipient: Option<String>,
    pub(in super::super) receive_from: Option<String>,
    pub(in super::super) receive_active: bool,
    pub(in super::super) builder: EnvelopeBuilder,
    pub(in super::super) status: SessionStatusState,
    pub(in super::super) last_status_ms: i64,
    pub(in super::super) mapper: HarnessLineMapper,
    pub(in super::super) tailer: Option<SessionTailer>,
    pub(in super::super) wrapper_session_id: String,
    pub(in super::super) harness_session_id: String,
    pub(in super::super) status_throttle_ms: i64,
    pub(in super::super) status_idle_ms: i64,
}
