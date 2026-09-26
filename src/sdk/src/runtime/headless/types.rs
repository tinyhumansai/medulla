//! Data types for the `headless` module.
#[allow(unused_imports)]
use super::*;
/// Timeouts bounding the two waits the driver performs. Both are generous by
/// default so a slow attach or a long cycle is not cut short; callers running
/// under a test harness pass shorter values.
#[derive(Debug, Clone, Copy)]
pub struct HeadlessOptions {
    /// How long to wait for the runtime to attach (reach a `Live`/no-stream
    /// state) before giving up.
    pub ready_timeout: Duration,
    /// How long to wait for the submitted instruction's cycle to finish before
    /// giving up.
    pub cycle_timeout: Duration,
}
/// Why a headless run failed, as an explicit SDK-boundary error type so a
/// caller (the `medulla run` wiring, a CI harness) can map each outcome to an
/// exit code or assertion by variant instead of matching display strings.
#[derive(Debug, thiserror::Error)]
pub enum HeadlessError {
    /// The runtime never reached a `Live` (attached) stream state within
    /// [`HeadlessOptions::ready_timeout`].
    #[error("timed out waiting for the runtime to attach")]
    AttachTimeout,
    /// The runtime latched unavailable (`Stalled`) before the instruction was
    /// submitted — a rejected handshake or version mismatch.
    #[error("core runtime is unavailable: {runtime}")]
    Unavailable {
        /// The runtime's [`Runtime::describe`] line, naming what was attached.
        runtime: String,
    },
    /// The runtime latched unavailable (`Stalled`) after the instruction was
    /// accepted but before its cycle ended.
    #[error("core runtime became unavailable mid-cycle: {runtime}")]
    UnavailableMidCycle {
        /// The runtime's [`Runtime::describe`] line, naming what was attached.
        runtime: String,
    },
    /// The runtime refused the submitted instruction — no cycle will ever
    /// start, so the run fails fast instead of waiting out the cycle timeout.
    #[error("the runtime rejected the instruction: {0}")]
    SubmitRejected(#[source] anyhow::Error),
    /// The submitted instruction's cycle did not finish within
    /// [`HeadlessOptions::cycle_timeout`].
    #[error("timed out waiting for the cycle to finish")]
    CycleTimeout,
    /// Writing an NDJSON line to the caller's `out` stream failed.
    #[error("failed to write the transcript stream: {0}")]
    Output(#[from] std::io::Error),
}
/// What one headless run settled to, for the caller's exit code / assertions.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HeadlessSummary {
    /// The cycle's pass count, from the terminal `cycle_end` event.
    pub pass_count: i64,
    /// How many event lines were streamed (excludes `ready`/`result`).
    pub events_streamed: usize,
}
