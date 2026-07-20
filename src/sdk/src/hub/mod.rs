//! The tiny.place task-sender hub — the outbound half of the harness plane.
//!
//! The [`daemon`](crate::daemon) (worker) only ever *receives* task frames; this
//! module *sends* them. [`TaskRunner`] dispatches a `task` frame to a remote
//! worker over Signal DMs and routes the worker's reply back, so a hosted
//! orchestrator can drive tiny.place workers it does not run locally. The
//! [`Relay`] seam abstracts the encrypted transport — production uses
//! [`SignalTransport`](crate::daemon::transport::SignalTransport); tests use a
//! fake — and the runner correlates concurrent dispatches by `correlationId`
//! over the one shared, destructively-drained inbox.

mod boot;
mod relay;
mod roster;
mod runner;
mod socket;
mod types;

#[cfg(test)]
mod tests;

pub use boot::{run_hub, start_hub, HubConfig, HubSession, WorkerSpec};
pub use relay::Relay;
pub use roster::{HubHandle, HubWorker};
pub use runner::TaskRunner;
pub use types::{RunError, TaskOutcome, TaskRequest};
