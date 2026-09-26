//! What a coding-agent harness is *working on*, in one vocabulary.
//!
//! Every harness shows more on its own screen than the transcript it streams:
//! Claude Code renders a live todo list and collapses each `Task` sub-agent to a
//! line, Codex prints a plan and a file-change summary, OpenCode tracks the same
//! todos under different tool names. Until now all of that arrived here as
//! anonymous `tool_call` events and was rendered as "ran a tool" — the operator
//! watching the fleet could see that something happened without ever seeing
//! what the agent thought it was doing.
//!
//! This module is the one place that knowledge is modelled. The per-provider
//! mappers ([`crate::daemon::mappers`]) recognize each harness's spelling and
//! emit the neutral event kinds named in [`kinds`]; [`WorkFold`] folds those
//! into a [`WorkSnapshot`]; and the UI renders the snapshot without knowing
//! which harness produced it.
//!
//! Split by responsibility: `types` holds the data model, `fold` the
//! stateful reduction. The event kinds are additive on the wire — a peer that
//! predates them decodes them as unknown events and ignores them, which is why
//! they can ship without a protocol version bump.

mod fold;
mod types;

#[cfg(test)]
mod tests;

pub use fold::WorkFold;
pub use types::{
    pull_request_label, FileChange, FileChangeKind, HarnessSessionInfo, RunOutcome, SubagentRun,
    SubagentStatus, TodoItem, WorkItemStatus, WorkSnapshot,
};

/// The semantic-event kinds this module understands.
///
/// These extend the published harness-event vocabulary (`user_prompt`,
/// `agent_message`, `tool_call`, …) rather than replacing any of it. They are
/// additive: a consumer that does not know a kind folds it to "unknown" and
/// drops it, so emitting them is safe against every existing peer.
pub mod kinds {
    /// The harness rewrote its todo list. Payload: `{ todos: [...], source }`.
    pub const TODO_UPDATE: &str = "todo_update";
    /// The harness declared a plan or goal. Payload: `{ goal, steps: [...] }`.
    pub const PLAN_UPDATE: &str = "plan_update";
    /// The harness delegated to a sub-agent. Payload:
    /// `{ call_id, agent_type, description, prompt }`.
    pub const SUBAGENT_START: &str = "subagent_start";
    /// The harness changed a file. Payload:
    /// `{ path, change, tool, additions, deletions }`.
    pub const FILE_CHANGE: &str = "file_change";
    /// The harness described its own session. Payload:
    /// `{ model, cwd, session_id, permission_mode, tools, slash_commands, mcp_servers }`.
    pub const SESSION_INFO: &str = "session_info";
    /// The harness run ended. Payload:
    /// `{ ok, duration_ms, num_turns, cost_usd, summary }`.
    pub const RUN_RESULT: &str = "run_result";

    /// Whether `kind` is one of the work kinds defined here.
    pub fn is_work_kind(kind: &str) -> bool {
        matches!(
            kind,
            TODO_UPDATE | PLAN_UPDATE | SUBAGENT_START | FILE_CHANGE | SESSION_INFO | RUN_RESULT
        )
    }
}
