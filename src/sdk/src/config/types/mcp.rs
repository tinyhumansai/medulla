//! The `mcp` section: what Medulla's own MCP server offers the harnesses it
//! spawns.
//!
//! Medulla drives Claude Code and Codex over ACP as a *client*, so the only way
//! it can hand a harness anything to call is to offer it an MCP server. That
//! server is this binary in a subprocess, and this section decides what the
//! subprocess is allowed to do on the harness's behalf.
//!
//! The settings here govern an *internal* surface. Nothing in this section
//! writes to another program's configuration, and nothing it enables is
//! reachable by a process Medulla did not spawn: the socket path and the grant
//! token are minted per session and handed to exactly one child. That is why
//! `fleetTools` can default on where a capability exposed to the whole machine
//! could not.

use super::*;

/// The default re-entry depth: the orchestrator dispatches, and the agents it
/// dispatched may each dispatch once more.
///
/// Two rather than one because a single level makes the fleet tools nearly
/// pointless — the orchestrator can already fan out, and the reason to give an
/// agent these tools is that it has context the orchestrator lacks. Two rather
/// than unbounded because every level is a real harness session costing real
/// money, and a tree nobody chose the shape of is a tree nobody can reason
/// about.
fn d_max_depth() -> u8 {
    2
}

/// The `mcp` section: Medulla's tool server, as offered to a spawned harness.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct McpSection {
    /// Whether a spawned harness is offered the `fleet_*` tools at all.
    ///
    /// On by default. Turning it off leaves the workflow tools untouched, so a
    /// harness can still author a graph; it simply cannot see or dispatch into
    /// the fleet it is running in.
    #[serde(default = "d_true")]
    pub fleet_tools: bool,
    /// How many levels deep a dispatch tree may go before `fleet_dispatch` is
    /// withheld.
    ///
    /// Counted from the operator's own turn: at `0` no spawned harness may
    /// dispatch, at `1` only those the orchestrator spawned directly, and so on.
    /// The limit is enforced against the grant the server minted, not against
    /// anything the harness reports about itself, so a model cannot talk its way
    /// past it.
    #[serde(default = "d_max_depth")]
    pub max_depth: u8,
    /// The most concurrent MCP-dispatched tasks one grant may hold.
    ///
    /// Absent follows `workflows.maxParallelAgents`, which already means "the
    /// most harness tasks one run may have in flight" and is sized by what the
    /// worker pool can actually serve. Set it here only to give MCP dispatch a
    /// different ceiling from workflow fan-out.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_in_flight: Option<usize>,
    /// Override the control socket path.
    ///
    /// Absent uses an account-scoped path under the Medulla home, which is what
    /// keeps two accounts on one machine from sharing a fleet. Treat a value
    /// here as untrusted configuration: it is validated before anything binds.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub socket_path: Option<String>,
}

impl Default for McpSection {
    fn default() -> Self {
        McpSection {
            fleet_tools: d_true(),
            max_depth: d_max_depth(),
            max_in_flight: None,
            socket_path: None,
        }
    }
}

impl McpSection {
    /// The in-flight ceiling for one grant, given the workflow fan-out limit.
    ///
    /// Falls back to `workflows.maxParallelAgents` so an operator who sized
    /// their worker pool once does not have to say the same number twice.
    pub fn effective_max_in_flight(&self, max_parallel_agents: usize) -> usize {
        self.max_in_flight.unwrap_or(max_parallel_agents).max(1)
    }
}
