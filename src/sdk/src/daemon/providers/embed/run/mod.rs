//! Drive one embedded agent turn with scoped progress and an idle watchdog.

mod events;
mod execution;
mod progress;
mod types;
mod watchdog;

#[cfg(test)]
mod tests;

pub use execution::run_local_task;

pub(super) use types::EventSink;

use crate::protocol::HarnessProvider;

use super::super::types::RunTaskOptions;

/// Whether `options` should run on the local harness rather than a spawned CLI.
///
/// A function rather than an inline `matches!` at the one call site, for the
/// same reason [`super::super::acp::uses_acp`] is one: the transport decisions
/// in [`super::super::execute::run_provider_task`] read as a list of questions,
/// and one of them phrased differently is one a reader has to stop at.
pub fn uses_local_harness(options: &RunTaskOptions) -> bool {
    options.provider == HarnessProvider::Openhuman
}
