//! The tool surface a local agent turn runs with.
//!
//! # Why Medulla owns these
//!
//! TinyAgents supplies the agent loop, the providers and the `Tool` trait, but
//! deliberately not a shell or a filesystem: what a tool is *allowed* to do
//! depends on the host's threat model, so a crate that shipped one would be
//! wrong for every host that disagreed. The embedded OpenHuman core used to
//! supply them here; owning them is the other half of dropping it.
//!
//! # Shape
//!
//! [`guard`] is the containment rule, [`fs`] the path-owning tools that enforce
//! it, and [`shell`] a plain supervised spawn that does not pretend to. Each is
//! rooted at one [`fs::Workspace`] handed in at construction rather than read
//! from process state, so two turns in one process cannot decide each other's
//! working directory.

pub mod env;
pub mod fs;
pub mod guard;
pub mod shell;

use std::sync::Arc;

use tinyagents::tool::Tool;

pub use fs::Workspace;

/// Every tool a local turn gets, rooted at one checkout.
///
/// `shell_timeout` is the per-command deadline; it is a parameter rather than a
/// constant because a workflow node's budget and an interactive turn's patience
/// are not the same number.
///
/// `env` becomes the shell's entire environment — see [`shell`] on why it is
/// passed rather than inherited.
pub fn all(
    workspace: Workspace,
    shell_timeout: std::time::Duration,
    env: std::collections::HashMap<String, String>,
) -> Vec<Arc<dyn Tool<()>>> {
    let mut tools = fs::all(workspace.clone());
    tools.extend(shell::all(workspace, shell_timeout, env::scrubbed(&env)));
    tools
}
