//! PTY-backed execution of delegated harness tasks.
//!
//! [`PtySessionExecutor`] is the public adapter. [`run`] owns its execution
//! behavior, while [`types`] owns the executor and session-planning data.

mod run;
#[cfg(test)]
mod tests;
mod types;

pub use run::agent_kind;
pub use types::PtySessionExecutor;
