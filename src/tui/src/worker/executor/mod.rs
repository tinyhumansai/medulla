//! PTY-backed execution of delegated harness tasks.
//!
//! [`PtySessionExecutor`] is the public adapter. [`run`] owns its dispatch
//! (timeout, retry, handoff), [`launch`] owns session planning and spawn,
//! [`turn`] owns transcript polling, [`hold`] owns queue/suspend/handback, and
//! [`types`] owns the executor and session-planning data.

mod hold;
mod launch;
mod run;
#[cfg(test)]
mod tests;
mod turn;
mod types;

pub use run::agent_kind;
pub use types::PtySessionExecutor;
