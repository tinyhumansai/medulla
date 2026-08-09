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
//! * [`with_progress_sink`] — scopes a sink around a future, via a
//!   `tokio::task_local` the core reads when it builds the agent for an
//!   `openhuman.inference_agent_chat` turn.
//!
//! `with_progress_sink` is the same mechanism as
//! [`crate::core_host::turn_cwd::with_turn_cwd`], so the two compose by
//! wrapping the same `invoke` future.
//!
//! The crate exports these as its whole public progress surface
//! (`openhuman_core::agent_progress`), so nothing here reaches through a
//! private module. Note the *crate* is `openhuman_core`; the module of the same
//! name one level in is the core's own internal tree.

pub use openhuman_core::agent_progress::{with_progress_sink, AgentProgress, ProgressSink};
