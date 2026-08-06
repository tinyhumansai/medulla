//! The task-sender hub — the outbound half of the harness plane.
//!
//! The [`daemon`](crate::daemon) (worker) only ever *receives* task frames; this
//! module *sends* them. [`TaskRunner`] dispatches a `task` frame over a
//! [`Bridge`](crate::bridge::Bridge) and routes the worker's reply back. A
//! [`LocalBridge`](crate::bridge::LocalBridge) keeps traffic in-process, while a
//! [`TinyplaceBridge`](crate::bridge::TinyplaceBridge) reaches remote workers
//! over encrypted host-link datagrams. The runner correlates concurrent dispatches by
//! `correlationId` over the one shared, destructively-drained inbox.

mod activity;
mod boot;
mod handle;
/// The handoff brief an operator leaves when giving a harness back. Public as a
/// module so its bounds keep their short names — `handoff::NOTE_MAX` reads, a
/// `HANDOFF_NOTE_MAX` flattened into the crate root does not.
pub mod handoff;
mod probe;
mod relay;
mod roster;
mod runner;
mod screens;
mod socket;
mod types;
mod workflows;

#[cfg(test)]
mod tests;

pub use activity::{ActivityLog, WorkerActivity};
pub use boot::{
    run_hub, start_hub, HubConfig, HubLinkConfig, HubLinkPeer, HubSession, WorkerSpec,
    DEFAULT_LOCAL_HUB_ADDRESS,
};
pub use handle::HubHandle;
pub use handoff::{HandoffControl, HarnessHandoff};
pub use relay::Relay;
pub use roster::{HubWorker, SharedLocalHosts};
pub use runner::TaskRunner;
pub use screens::{ScreenStore, WatchedScreen};
pub use types::{stderr_log, HubLog, RosterSink, RunError, TaskOutcome, TaskRequest};
pub use workflows::{WorkflowBridge, WorkflowPlane};
