//! The single import seam onto the vendored core's per-turn progress contract.
//!
//! Everything this provider needs from the core's progress plumbing is named
//! here, once, and re-exported. The vendored crate is upstream code on its own
//! release cadence, so the path these items live at is not ours to pin: when it
//! moves, this file is the only edit, and no logic module has to be reread to be
//! sure nothing else reached into the core.
//!
//! # The contract
//!
//! * [`AgentProgress`] — the enum the agent turn loop emits as it works.
//! * [`ProgressSink`] — the channel the core sends those events on.
//!
//! `with_progress_sink` used to be named here too: the sink was scoped around
//! the dispatch by hand, through a `tokio::task_local` the core read when it
//! built the turn's agent. `Turn::on_progress` takes the sink as an argument
//! now and enters that scope itself, so this provider no longer has to know the
//! mechanism exists — only the two types the events arrive as.
//!
//! [`crate::core_host::turn_cwd::with_turn_cwd`] is still scoped by hand, and
//! deliberately: it is read by *Medulla's* process-global lifecycle hooks, not
//! by the core, so there is nothing upstream that could take it as an argument.
//!
//! The crate exports these as its whole public progress surface
//! (`openhuman_core::agent_progress`), so nothing here reaches through a
//! private module. Note the *crate* is `openhuman_core`; the module of the same
//! name one level in is the core's own internal tree.

pub use openhuman_core::agent_progress::{AgentProgress, ProgressSink};
