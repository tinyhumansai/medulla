//! Codex over a shared `codex app-server` process.
//!
//! The third harness transport, alongside the per-task CLI fork and ACP. Where
//! those spawn a process per task, this opens a *thread* on a pooled one, so a
//! fan-out of lanes costs one Codex runtime rather than one each. That is the
//! whole of why it exists, and it is why workflows — which fan out by
//! construction — are what it was built for.
//!
//! Selected by naming the `codex-server` harness, which reaches here as
//! [`HarnessTransport::AppServer`](crate::protocol::HarnessTransport::AppServer)
//! on the task frame, or by
//! [`HARNESS_TRANSPORT_ENV`] for callers with no frame to state it on.
//!
//! Split by responsibility: `execution` owns thread setup and the turn loop,
//! `fold` translates app-server notifications into Medulla's semantic events.
//! The pooled client itself is [`crate::codex_app_server`].

mod execution;
mod fold;

#[cfg(test)]
mod tests;

pub(super) use execution::uses_app_server;
pub use execution::{run_codex_server_task, HARNESS_TRANSPORT_ENV};
