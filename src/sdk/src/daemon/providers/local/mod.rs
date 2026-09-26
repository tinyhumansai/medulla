//! Running a task on the local agent harness, in this process.
//!
//! Every other provider in this module is a *CLI*: Medulla resolves a binary,
//! spawns it, and folds its JSONL. This one has no binary to spawn — the agent
//! loop is [`crate::agent`], linked into this process. So the adapter replaces
//! the whole spawn seam rather than parameterising it: no argv, no environment
//! scrubbing, no stdout to parse, and no child to kill.
//!
//! # What replaced what
//!
//! This used to dispatch `openhuman.inference_agent_chat` into an embedded
//! OpenHuman core, which meant a node running a model in a loop with three
//! tools pulled a whole desktop product — memory engine, channel providers,
//! cron scheduler — into the binary. The loop is tinyagents now and the tools
//! are Medulla's; see [`crate::agent`] for the split and
//! [`crate::agent::tools`] for why the tool surface is the host's to own.
//!
//! Two properties changed with it, and an operator will notice both:
//!
//! * **The turn no longer shares a core's state.** It has no access to the
//!   operator's OpenHuman memory, flows or credentials, because there is no
//!   core to hold them. What it gets is the checkout and the route.
//! * **There is no approval gate.** OpenHuman refused external-effect tools
//!   from an unlabelled caller and parked them for approval otherwise; the
//!   local harness runs what the model asks for. See
//!   [`crate::agent::tools::shell`], which says so at the call site rather than
//!   implying a boundary that is not there.
//!
//! It is still never auto-selected — see [`super::detect::detect_providers`].
//! A node gets it by naming it.
//!
//! # What a turn is
//!
//! One [`crate::agent::turn::run`]: a bounded model/tool loop over the node's
//! checkout. `thread_id` carries continuity and is what
//! [`RunTaskOptions::resume_session_id`] resolves to, so a bounded workflow node
//! gets a fresh thread and a resumed conversation keeps its own — exactly as the
//! CLI providers' session ids behave.
//!
//! That continuity is [`crate::agent::history`]: the transcript a turn produced
//! is stored under its thread id and replayed into the next turn naming it. The
//! embedded core supplied this from its own session store; the id would
//! otherwise name a conversation that existed nowhere.
//!
//! # What is deliberately absent
//!
//! *Hooks, managed skills and MCP tools.* [`crate::harness_hooks`] installs all
//! three onto a *child's* command line, and none survives having no child. The
//! embedded core carried hooks through a process-global registration instead;
//! that mechanism went with it, so a hook declared for this provider does not
//! fire. Re-adding them means an in-process hook seam on the tool dispatch,
//! which belongs with whoever needs it rather than as a stub that silently
//! does nothing.
//!
//! *A model of its own.* The turn runs on whatever the operator chose — see
//! [`model`] for every route to that choice and the order they resolve in, and
//! [`router`] for the endpoint and credential that make a chosen model
//! reachable.

mod model;
mod router;
mod run;

#[cfg(test)]
mod tests;

pub use model::effective_model;
pub use router::embedded_route;
pub use run::{run_local_task, uses_local_harness};
