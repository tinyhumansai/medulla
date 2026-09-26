//! Joining a run's durable record to the harness sessions it has in flight.
//!
//! `workflow_run_get` reads the store, and the store only learns about a node
//! once that node has *finished*: a step is appended when the engine reports it
//! settled. So for the twenty minutes an `agent` node spends being a whole
//! coding session, the record says nothing about it at all — the caller sees a
//! run that is `running` with one fewer step than it expects and has no way to
//! tell a harness working from a harness wedged.
//!
//! The missing half is not in the store, and it is not in the engine either. It
//! is in whichever dispatcher put the session out. This module reaches for it —
//! in two places, because there are two dispatchers.
//!
//! # The join
//!
//! Every `agent` dispatch is tagged `wf:{run_id}:{route}#{sequence}` (see
//! [`crate::flow_engine::caps::agent`]), so that id is the whole of the
//! correlation. Two sources report ids under it:
//!
//! - **The fleet roster.** The hub records which worker each outstanding task
//!   id is running on, so filtering the roster's `running` lists by the
//!   `wf:{run_id}:` prefix leaves this run's live sessions attributed to
//!   machines. This is the only view of a run executing in *another* process.
//! - **This process's own dispatch registry**
//!   ([`crate::workflows::run::dispatches`]). A run started here dispatches
//!   onto a [`LocalWorkflowHost`](crate::workflows::local) it owns and drops
//!   with itself; that host never registers with the outer control plane, so
//!   the roster has no row for it and no task id under this prefix. Without
//!   this half, the runs an MCP caller most often asks about — the ones it
//!   started itself — would read as doing nothing for the whole of a long
//!   coding step.
//!
//! [`detail::merge`] folds the two, preferring the roster's row for a task id
//! both report, since it names the machine that actually took the work.
//!
//! # What this cannot see
//!
//! The harness *transcript* — what the session is typing, which files it has
//! touched, how far through it thinks it is. The hub carries a work snapshot on
//! each frame it observes, but no control-plane op exposes the activity log, so
//! the MCP server (a subprocess with no hub of its own) cannot read it. The
//! honest answer is therefore "this worker is running this step of your run",
//! not a progress bar, and the tool says so rather than implying more.
//!
//! Nor can it see the harness sessions of a run executing in another process
//! that dispatches onto its own embedded host — a run started from the TUI, say.
//! Both halves of the join miss that case: the local registry belongs to this
//! process, and the roster does not know that host either. `executingHere` is
//! likewise a fact about *this* server only, which is why the harness half is
//! reported independently of it.

mod detail;
mod types;

#[cfg(test)]
mod tests;

pub(crate) use detail::detail;
