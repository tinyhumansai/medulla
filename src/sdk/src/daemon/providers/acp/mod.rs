//! ACP-backed harness execution.
//!
//! Execution owns client/session setup, folding translates provider updates
//! into Medulla events, and types retains private stream state.

mod execution;
mod fold;
mod types;

#[cfg(test)]
mod tests;

pub(super) use execution::uses_acp;
pub use execution::{run_acp_task, HARNESS_PROTOCOL_ENV};
