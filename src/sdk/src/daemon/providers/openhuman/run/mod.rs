//! Executing one task as an in-process OpenHuman agent turn.
//!
//! The turn itself is a single core call; everything around it is supervision:
//! [`core_contract`] names what the core promises to stream back,
//! [`types`] owns the per-turn state the fold and the sink share, [`progress`]
//! folds that stream into Medulla's event vocabulary, [`events`] implements the
//! outgoing transcript sink, [`watchdog`] keeps the turn alive for as long as
//! it is genuinely working, and [`execution`] ties them together into one task.
//!
//! What stays here is the routing question — whether a given [`RunTaskOptions`]
//! belongs on this path at all ([`uses_embedded_core`]) — so the module reads
//! as a launcher for code that lives in its submodules.

mod core_contract;
mod events;
mod execution;
mod progress;
mod types;
mod watchdog;

#[cfg(test)]
mod tests;

pub(super) use execution::reply_text;
pub(super) use execution::run_openhuman_task;
pub(super) use types::{EventSink, ProgressFold};

use crate::protocol::HarnessProvider;

use super::super::types::RunTaskOptions;

/// Whether `options` should run on the embedded core rather than a spawned CLI.
///
/// A function rather than an inline `matches!` at the one call site, for the
/// same reason [`super::super::acp::uses_acp`] is one: the transport decisions
/// in [`super::super::execute::run_provider_task`] read as a list of questions,
/// and one of them phrased differently is one a reader has to stop at.
pub fn uses_embedded_core(options: &RunTaskOptions) -> bool {
    options.provider == HarnessProvider::Openhuman
}