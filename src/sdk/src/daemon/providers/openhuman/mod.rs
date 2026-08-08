//! Running a task on the embedded OpenHuman core, in this process.
//!
//! Every other provider in this module is a *CLI*: Medulla resolves a binary,
//! spawns it, and folds its JSONL. OpenHuman has no binary to spawn — it is
//! already here, linked into this process and booted once
//! ([`crate::core_host::shared`]). So this adapter replaces the whole spawn
//! seam rather than parameterising it: no argv, no environment scrubbing, no
//! stdout to parse, and no child to kill.
//!
//! # Why it is a harness at all
//!
//! Until now OpenHuman was excluded from dispatch on the grounds that it "owns
//! its agent loop and shares the host's core state rather than accepting
//! coding-task frames". Both halves are still true; what changed is that
//! neither is a reason to refuse. Owning its own agent loop is what every
//! harness here does — Claude Code owns one too, behind a process boundary
//! instead of a function call. Sharing the host's core state is a *property* of
//! this provider, not a disqualification: it is precisely why an operator would
//! choose it for a node, since the turn runs with the memory, the flows, and
//! the credentials the operator already set up.
//!
//! What remains true is that OpenHuman is not interchangeable with a coding
//! CLI, which is why it is never auto-selected — see
//! [`super::detect::detect_providers`]. A node gets it by naming it.
//!
//! # What a turn is
//!
//! One call to `openhuman.inference_agent_chat`, which is the core's own agent
//! loop: its tools, its memory, its approval gate. `thread_id` carries
//! continuity, and it is what [`RunTaskOptions::resume_session_id`] resolves to
//! — so a bounded workflow node gets a fresh thread and a resumed conversation
//! keeps its own, exactly as the CLI providers' session ids behave.
//!
//! # What is deliberately absent
//!
//! *Hooks.* [`crate::harness_hooks`] installs lifecycle hooks onto a child's
//! argv, and there is no child. A turn here is a function call in this process,
//! so the thing hooks exist to observe — a separate program doing work on its
//! own — is not happening. Reported once at dispatch rather than left for an
//! operator to infer from an empty hook log.
//!
//! *Managed skills and MCP tools.* Same reason, and in this case no loss: the
//! core reaches Medulla through the process it is already inside.

mod run;

#[cfg(test)]
mod tests;

pub use run::{run_openhuman_task, uses_embedded_core};
