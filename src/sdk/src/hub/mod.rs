//! The task-sender hub — the outbound half of the harness plane.
//!
//! The [`daemon`](crate::daemon) (worker) only ever *receives* task frames; this
//! module *sends* them. [`TaskRunner`] dispatches a `task` frame over a
//! [`Bridge`](crate::bridge::Bridge) and routes the worker's reply back. A
//! [`LocalBridge`](crate::bridge::LocalBridge) keeps traffic in-process, while a
//! [`TinyplaceBridge`](crate::bridge::TinyplaceBridge) reaches remote workers
//! over encrypted tiny.place DMs. The runner correlates concurrent dispatches by
//! `correlationId` over the one shared, destructively-drained inbox.

mod activity;
mod boot;
mod handle;
mod relay;
mod roster;
mod runner;
mod socket;
mod types;

#[cfg(test)]
mod tests;

pub use activity::{ActivityLog, WorkerActivity};
pub use boot::{run_hub, start_hub, HubConfig, HubSession, WorkerSpec, DEFAULT_LOCAL_HUB_ADDRESS};
pub use handle::HubHandle;
pub use relay::Relay;
pub use roster::HubWorker;
pub use runner::TaskRunner;
pub use types::{stderr_log, HubLog, RosterSink, RunError, TaskOutcome, TaskRequest};
