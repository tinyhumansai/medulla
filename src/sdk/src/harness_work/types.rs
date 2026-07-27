//! The provider-neutral work model: what every coding-agent harness shows on
//! its own screen, reduced to one vocabulary.
//!
//! Each harness has its own name for the same handful of ideas — Claude Code
//! writes a todo list through `TodoWrite`, Codex emits a `todo_list` item,
//! OpenCode calls the tool `todowrite` — and each serializes it differently.
//! These types are the shape those all fold onto, so a consumer renders one
//! thing rather than three. Every field is optional or defaulted: a harness that
//! never reports a plan leaves the plan empty rather than forcing an invented
//! value, and a harness we have not taught yet contributes nothing instead of
//! breaking the fold.
//!
//! Wire form is snake_case, matching the semantic-event payload structs these
//! ride inside (`tool_call`, `tool_result`, …) rather than the camelCase task
//! frames.

use serde::{Deserialize, Serialize};

/// Lifecycle of one todo/plan item, normalized across harnesses.
///
/// Claude writes `pending|in_progress|completed`, Codex writes
/// `pending|in_progress|completed` on its todo items, OpenCode writes the same
/// three plus `cancelled`. Anything unrecognized parses as
/// [`Pending`](WorkItemStatus::Pending) — an unknown state is safest read as
/// "not done yet", since claiming completion nobody reported is the worse error.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkItemStatus {
    /// Queued, not started.
    #[default]
    Pending,
    /// Currently being worked.
    InProgress,
    /// Finished.
    Completed,
    /// Abandoned.
    Cancelled,
}

impl WorkItemStatus {
    /// Parse a harness-reported status string, defaulting to
    /// [`Pending`](WorkItemStatus::Pending) for anything unrecognized.
    pub fn from_wire(value: &str) -> Self {
        match value.trim().to_ascii_lowercase().as_str() {
            "in_progress" | "in-progress" | "active" | "running" | "started" => {
                WorkItemStatus::InProgress
            }
            "completed" | "complete" | "done" | "finished" => WorkItemStatus::Completed,
            "cancelled" | "canceled" | "skipped" | "abandoned" => WorkItemStatus::Cancelled,
            _ => WorkItemStatus::Pending,
        }
    }

    /// The lowercase wire label for this status.
    pub fn label(self) -> &'static str {
        match self {
            WorkItemStatus::Pending => "pending",
            WorkItemStatus::InProgress => "in_progress",
            WorkItemStatus::Completed => "completed",
            WorkItemStatus::Cancelled => "cancelled",
        }
    }

    /// The checkbox glyph for this status, matching the vocabulary the harnesses
    /// use on their own screens.
    pub fn glyph(self) -> char {
        match self {
            WorkItemStatus::Pending => '☐',
            WorkItemStatus::InProgress => '▶',
            WorkItemStatus::Completed => '☑',
            WorkItemStatus::Cancelled => '⊘',
        }
    }

    /// The display colour name for this status, in the palette the TUI's
    /// `color()` mapping understands.
    pub fn color(self) -> &'static str {
        match self {
            WorkItemStatus::Pending => "gray",
            WorkItemStatus::InProgress => "yellow",
            WorkItemStatus::Completed => "green",
            WorkItemStatus::Cancelled => "gray",
        }
    }

    /// Whether this status is terminal (no further work expected).
    pub fn is_terminal(self) -> bool {
        matches!(self, WorkItemStatus::Completed | WorkItemStatus::Cancelled)
    }
}

/// One entry on a harness's own todo list.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct TodoItem {
    /// The item text as the harness wrote it.
    pub text: String,
    /// Normalized lifecycle status.
    pub status: WorkItemStatus,
    /// The present-participle form Claude Code shows while an item is running
    /// ("Running tests"), when the harness supplies one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub active_form: Option<String>,
}

impl TodoItem {
    /// What to display for this item: the active form while it is in progress
    /// (that is what the harness itself shows), else the plain text.
    pub fn display(&self) -> &str {
        match (&self.active_form, self.status) {
            (Some(form), WorkItemStatus::InProgress) if !form.trim().is_empty() => form,
            _ => &self.text,
        }
    }
}

/// How a delegated sub-agent run ended, or that it has not.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SubagentStatus {
    /// Still running.
    #[default]
    Running,
    /// Returned successfully.
    Done,
    /// Returned an error.
    Failed,
}

impl SubagentStatus {
    /// The display colour name for this status.
    pub fn color(self) -> &'static str {
        match self {
            SubagentStatus::Running => "yellow",
            SubagentStatus::Done => "green",
            SubagentStatus::Failed => "red",
        }
    }

    /// The lane-marker glyph for this status.
    pub fn glyph(self) -> char {
        match self {
            SubagentStatus::Running => '●',
            SubagentStatus::Done => '✓',
            SubagentStatus::Failed => '✗',
        }
    }
}

/// One sub-agent the harness spawned — Claude Code's `Task` tool, OpenCode's
/// `task`, or an MCP-side agent call.
///
/// This is the layer the operator most often cannot see: a harness fans work out
/// to several sub-agents and its own screen shows one collapsed line each. The
/// run is keyed by the tool `call_id`, which is what its result comes back on.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct SubagentRun {
    /// The spawning tool call's id — the key the result arrives under.
    pub call_id: String,
    /// The sub-agent type the harness was asked for (`general-purpose`,
    /// `code-reviewer`, …), when it named one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent_type: Option<String>,
    /// The short description of the delegated work.
    #[serde(default)]
    pub description: String,
    /// The full prompt handed to the sub-agent, bounded upstream.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prompt: Option<String>,
    /// Current status.
    pub status: SubagentStatus,
    /// The sub-agent's returned text, once it finished.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub result: Option<String>,
    /// Epoch ms when the spawn was observed.
    #[serde(default)]
    pub started_at: i64,
    /// Epoch ms when the result was observed, while it is still running.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ended_at: Option<i64>,
}

impl SubagentRun {
    /// The best one-line label for this run: its description, else its type,
    /// else the bare call id.
    pub fn label(&self) -> &str {
        if !self.description.trim().is_empty() {
            return &self.description;
        }
        match self.agent_type.as_deref() {
            Some(kind) if !kind.trim().is_empty() => kind,
            _ => &self.call_id,
        }
    }
}

/// What a harness did to a file.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FileChangeKind {
    /// The file was created.
    Added,
    /// The file was edited in place.
    #[default]
    Modified,
    /// The file was removed.
    Deleted,
}

impl FileChangeKind {
    /// Parse a harness-reported change kind, defaulting to
    /// [`Modified`](FileChangeKind::Modified).
    pub fn from_wire(value: &str) -> Self {
        match value.trim().to_ascii_lowercase().as_str() {
            "add" | "added" | "create" | "created" | "new" => FileChangeKind::Added,
            "delete" | "deleted" | "remove" | "removed" => FileChangeKind::Deleted,
            _ => FileChangeKind::Modified,
        }
    }

    /// The single-character marker for this kind, as a diff would write it.
    pub fn marker(self) -> char {
        match self {
            FileChangeKind::Added => 'A',
            FileChangeKind::Modified => 'M',
            FileChangeKind::Deleted => 'D',
        }
    }

    /// The display colour name for this kind.
    pub fn color(self) -> &'static str {
        match self {
            FileChangeKind::Added => "green",
            FileChangeKind::Modified => "yellow",
            FileChangeKind::Deleted => "red",
        }
    }
}

/// One file the harness touched, folded across repeated edits.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct FileChange {
    /// The path as the harness reported it.
    pub path: String,
    /// What happened to it.
    pub kind: FileChangeKind,
    /// How many separate edits landed on this path.
    #[serde(default)]
    pub edits: i64,
    /// Lines added, when the harness reported a diff stat.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub additions: Option<i64>,
    /// Lines removed, when the harness reported a diff stat.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub deletions: Option<i64>,
    /// Epoch ms of the most recent edit.
    #[serde(default)]
    pub at: i64,
}

/// The harness's own description of the session it is running.
///
/// Claude Code announces all of this on its `system`/`init` record; the other
/// harnesses report parts of it or none. Absent fields mean "not reported".
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct HarnessSessionInfo {
    /// The model the harness is running.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    /// The working directory the harness is rooted at.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cwd: Option<String>,
    /// The harness's own session id.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
    /// The permission mode the harness was started in.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub permission_mode: Option<String>,
    /// Tool names the harness has available.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tools: Vec<String>,
    /// Slash commands the harness exposes.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub slash_commands: Vec<String>,
    /// MCP servers the harness connected to.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub mcp_servers: Vec<String>,
}

impl HarnessSessionInfo {
    /// Whether anything at all was reported (an all-absent info renders nothing).
    pub fn is_empty(&self) -> bool {
        self.model.is_none()
            && self.cwd.is_none()
            && self.session_id.is_none()
            && self.permission_mode.is_none()
            && self.tools.is_empty()
            && self.slash_commands.is_empty()
            && self.mcp_servers.is_empty()
    }
}

/// How a whole harness run ended, from the terminal record it prints.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct RunOutcome {
    /// Whether the run reported success.
    pub ok: bool,
    /// Wall-clock duration in milliseconds, when reported.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub duration_ms: Option<i64>,
    /// Assistant turns taken, when reported.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub num_turns: Option<i64>,
    /// Billed cost in USD, when reported.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cost_usd: Option<f64>,
    /// The harness's own closing summary text, bounded upstream.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub summary: Option<String>,
}

/// Everything the harness is showing on its own screen, in one snapshot.
///
/// Built by folding a session's semantic events ([`WorkFold`](super::WorkFold)).
/// A snapshot with nothing in it is the normal state for a harness that reports
/// none of this, and [`is_empty`](WorkSnapshot::is_empty) lets a renderer skip
/// the whole surface rather than draw empty headings.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct WorkSnapshot {
    /// The stated goal of the current work, when the harness declared one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub goal: Option<String>,
    /// The current plan's steps, most recently declared.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub plan: Vec<TodoItem>,
    /// The live todo list, latest write wins.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub todos: Vec<TodoItem>,
    /// Sub-agents spawned in this session, in spawn order.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub subagents: Vec<SubagentRun>,
    /// Files touched, in first-touch order.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub files: Vec<FileChange>,
    /// What the harness said about itself.
    #[serde(default, skip_serializing_if = "HarnessSessionInfo::is_empty")]
    pub info: HarnessSessionInfo,
    /// How the run ended, once it has.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub outcome: Option<RunOutcome>,
}

impl WorkSnapshot {
    /// Whether the snapshot carries nothing worth rendering.
    pub fn is_empty(&self) -> bool {
        self.goal.is_none()
            && self.plan.is_empty()
            && self.todos.is_empty()
            && self.subagents.is_empty()
            && self.files.is_empty()
            && self.info.is_empty()
            && self.outcome.is_none()
    }

    /// `(completed, total)` over the todo list, counting cancelled items as
    /// settled — a cancelled item is not outstanding work, and showing 3/7 when
    /// four were dropped describes a queue that no longer exists.
    pub fn todo_progress(&self) -> (usize, usize) {
        let done = self.todos.iter().filter(|t| t.status.is_terminal()).count();
        (done, self.todos.len())
    }

    /// The item the harness says it is working on right now, if any.
    pub fn current_todo(&self) -> Option<&TodoItem> {
        self.todos
            .iter()
            .find(|t| t.status == WorkItemStatus::InProgress)
    }

    /// How many sub-agents are still running.
    pub fn running_subagents(&self) -> usize {
        self.subagents
            .iter()
            .filter(|s| s.status == SubagentStatus::Running)
            .count()
    }
}
