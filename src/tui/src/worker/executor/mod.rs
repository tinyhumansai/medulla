//! PTY-backed execution of delegated harness tasks.
//!
//! [`PtySessionExecutor`] is the public adapter. [`run`] owns its execution
//! behavior, [`probe`] owns deciding which session serves a task, [`hold`] owns
//! what happens when an operator is in the way — queue, suspend, hand back —
//! and [`types`] owns the executor and session-planning data.

mod hold;
mod probe;
mod run;
#[cfg(test)]
mod tests;
mod types;

pub use run::agent_kind;
pub use types::PtySessionExecutor;
/// Visible to the crate's own executor tests, which assert that the blocking
/// half of the session decision really does leave the runtime.
#[cfg(all(test, unix))]
pub(in crate::worker) use types::SessionProbe;
