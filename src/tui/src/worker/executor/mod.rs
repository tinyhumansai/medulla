//! PTY-backed execution of delegated harness tasks.
//!
//! [`PtySessionExecutor`] is the public adapter. [`run`] owns its dispatch
//! (timeout, retry, handoff), [`probe`] owns deciding which session serves a
//! task, [`launch`] owns the fresh-harness spawn, [`turn`] owns transcript
//! polling, [`hold`] owns queue/suspend/handback, and [`types`] owns the
//! executor and session-planning data.

mod hold;
mod launch;
mod probe;
mod run;
#[cfg(test)]
mod tests;
mod turn;
mod types;

pub use run::agent_kind;
pub use types::PtySessionExecutor;
/// Visible to the crate's own executor tests, which assert that the blocking
/// half of the session decision really does leave the runtime.
#[cfg(all(test, unix))]
pub(in crate::worker) use types::SessionProbe;
