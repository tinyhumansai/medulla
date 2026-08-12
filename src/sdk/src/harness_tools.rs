//! Whether a harness Medulla launches receives Medulla's own MCP tools.
//!
//! Most harnesses Medulla starts should get them: an operator's session is
//! meant to be able to reach `workflow_run` and the `fleet_*` verbs, and the
//! managed skills that name the installed workflows exist precisely so the
//! model knows those tools are there.
//!
//! A harness serving a workflow `agent` node is the exception, and it is the
//! only one. Such a harness *is* a step of a workflow that is already running.
//! Handing it `workflow_run` lets one node start another graph — recursively,
//! without the loop bound, the approval gates, or the concurrency budget the
//! engine applies to its own nodes — and handing it the `fleet_*` verbs lets it
//! dispatch to the very worker pool the run it belongs to is competing for. The
//! node was given an instruction and a workspace; orchestration is the graph's
//! job, not its steps'.
//!
//! # How the decision travels
//!
//! Through the child's environment, because that is the one carrier every
//! launch seam already shares: the headless CLI spawn, the pooled app-server,
//! the ACP client, and the TUI's PTY launcher all build a child environment and
//! none of them share a call signature. A field on `RunTaskOptions` would reach
//! the first three and not the fourth.
//!
//! # Why an environment variable is enough
//!
//! This marker is *advisory*, and deliberately so — it is not what makes the
//! withholding safe. What makes it safe is that the caller which sets it also
//! declines to mint a fleet grant at all
//! ([`crate::control_socket::grants`]), so there is no capability for a child
//! to redeem however it rewrites its own environment. The marker's job is to
//! keep the *rest* of the launch consistent with that decision: not to register
//! an MCP server whose tools would fail unauthenticated, and not to render
//! skills that instruct the model to call a tool which is not there. A session
//! told to run `workflow_run` and served no such tool is worse than one told
//! nothing at all — it spends a turn discovering the gap.

use std::collections::HashMap;

/// The environment variable naming the tool surface a launch gets.
///
/// Only [`WITHHELD`] is meaningful; any other value, and the variable's
/// absence, mean the ordinary surface. Read that way round on purpose — a new
/// build must not withhold tools because it met a value an older one wrote.
pub const HARNESS_TOOLS_ENV: &str = "MEDULLA_HARNESS_TOOLS";

/// The value that withholds Medulla's tools from a launch.
pub const WITHHELD: &str = "none";

/// Mark `env` so the launch it describes receives no Medulla tools.
///
/// Also clears the two things that would otherwise re-introduce a tool surface
/// underneath the marker: the workflow tool-mode selector, which a launch seam
/// reads as "attach the server in this mode", and an inherited parent grant,
/// which a nested launch would exchange for a child grant of its own.
///
/// Clearing rather than trusting inheritance, for the reason
/// [`crate::daemon::task_loop`] clears the same keys: one process serves
/// launches under several policies, so a value left over from its own
/// environment could only ever be right for one of them.
pub fn withhold(env: &mut HashMap<String, String>) {
    env.insert(HARNESS_TOOLS_ENV.to_string(), WITHHELD.to_string());
    #[cfg(feature = "workflows")]
    env.remove(crate::mcp::TOOL_MODE_ENV);
    env.remove(crate::control_socket::MCP_SOCKET_ENV);
    env.remove(crate::control_socket::MCP_GRANT_ENV);
    env.remove(crate::control_socket::MCP_PARENT_SOCKET_ENV);
    env.remove(crate::control_socket::MCP_PARENT_GRANT_ENV);
}

/// Whether the launch this environment describes receives no Medulla tools.
///
/// Every attach seam consults this before registering the MCP server or
/// rendering managed skills.
pub fn withheld(env: &HashMap<String, String>) -> bool {
    env.get(HARNESS_TOOLS_ENV).map(String::as_str) == Some(WITHHELD)
}

#[cfg(test)]
#[path = "harness_tools_tests.rs"]
mod tests;
