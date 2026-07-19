//! Data model for the Agents view-model: the lane/task/turn structs, the role
//! and status enums with their trivial classification impls, the pre-styled
//! display [`Line`], and the [`AgentRow`] list-row enum. All behaviour lives in
//! the sibling logic modules; this file holds only the shapes and their trivial
//! accessors.

use crate::runtime::AgentDescriptor;

/// A cognitive tier / worker classification for a lane. Drives the lane colour
/// and whether the lane is a delegatable agent or a graph-invoked function.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AgentRole {
    /// The orchestrator tier.
    Orchestrator,
    /// The reasoning tier.
    Reasoning,
    /// The compress/summarizer function tier.
    Compress,
    /// A delegated worker / roster agent / session lane.
    Worker,
}

impl AgentRole {
    /// The display colour for this role.
    pub fn color(self) -> &'static str {
        match self {
            AgentRole::Orchestrator | AgentRole::Reasoning => "yellow",
            AgentRole::Compress => "blue",
            AgentRole::Worker => "magenta",
        }
    }
    /// A real (delegatable) agent, or a graph-invoked function (`compress`).
    pub fn is_function(self) -> bool {
        matches!(self, AgentRole::Compress)
    }
}

/// Terminal-or-running status of a delegated task lane.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TaskStatus {
    /// The task is still running.
    Running,
    /// The task completed successfully.
    Done,
    /// The task failed.
    Failed,
    /// §3.3(3): the wire keeps `cancelled` distinct from `failed`; the fold does too.
    Cancelled,
}

impl TaskStatus {
    /// Map a `task_complete` wire status onto the lane status. Unknown → Running is
    /// never produced here (a completion is terminal); an unrecognized string falls
    /// back to `failed` rather than silently reading as done.
    pub fn from_wire(s: &str) -> TaskStatus {
        match s {
            "done" => TaskStatus::Done,
            "cancelled" => TaskStatus::Cancelled,
            _ => TaskStatus::Failed,
        }
    }
    /// The lowercase wire label for this status.
    pub fn label(self) -> &'static str {
        match self {
            TaskStatus::Running => "running",
            TaskStatus::Done => "done",
            TaskStatus::Failed => "failed",
            TaskStatus::Cancelled => "cancelled",
        }
    }
    /// The display colour for this status.
    pub fn color(self) -> &'static str {
        match self {
            TaskStatus::Running => "yellow",
            TaskStatus::Done => "green",
            TaskStatus::Failed => "red",
            TaskStatus::Cancelled => "gray",
        }
    }
}

/// One folded turn in a lane or task transcript.
#[derive(Debug, Clone, Default)]
pub struct TurnBlock {
    /// Wire timestamp (ms) of the turn.
    pub at: i64,
    /// The turn header line.
    pub header: String,
    /// Optional colour override for the header.
    pub header_color: Option<String>,
    /// Optional reasoning/thinking body.
    pub reasoning: Option<String>,
    /// Optional output content body.
    pub content: Option<String>,
    /// Rendered tool-call lines.
    pub tools: Vec<String>,
}

/// The per-task state accumulated for an agent-identity lane.
#[derive(Debug, Clone)]
pub struct TaskState {
    /// The wire task id.
    pub task_id: String,
    /// Current status.
    pub status: TaskStatus,
    /// Number of folded turns.
    pub turns: usize,
    /// Timestamp (ms) of the last activity.
    pub last_at: i64,
    /// The folded turn blocks for this task.
    pub turn_blocks: Vec<TurnBlock>,
    /// A pending `task_attention` prompt (reason · content), cleared on completion.
    pub attention: Option<String>,
    /// The `questionId` of a pending `task_attention` — the handle `question.answer`
    /// needs. `None` when the task has no open question.
    pub question_id: Option<String>,
}

/// One lane in the Agents view: a cognitive tier, a roster/worker agent, an
/// anonymous task, or a peer session.
#[derive(Debug, Clone)]
pub struct AgentLane {
    /// The lane-unique key.
    pub key: String,
    /// The display label.
    pub label: String,
    /// The lane's role/classification.
    pub role: AgentRole,
    /// The lane-level transcript turns.
    pub turns: Vec<TurnBlock>,
    /// Timestamp (ms) of the last activity.
    pub last_at: i64,
    /// Per-task state for agent-identity lanes.
    pub tasks: Vec<TaskState>,
    /// Last-known context token count, if reported.
    pub context_tokens: Option<i64>,
    /// A harness label tag, if learned.
    pub harness_label: Option<String>,
    /// The backing agent id, if any.
    pub agent_id: Option<String>,
    /// The backing session id, for session lanes.
    pub session_id: Option<String>,
    /// The parent agent id, for session lanes grouped under a machine.
    pub parent_agent_id: Option<String>,
    /// The roster descriptor, for roster-seeded lanes.
    pub descriptor: Option<AgentDescriptor>,
    /// Count of currently-active tasks.
    pub active_tasks: i64,
}

/// A single pre-styled display row.
#[derive(Debug, Clone, Default)]
pub struct Line {
    /// The row text.
    pub text: String,
    /// Optional colour.
    pub color: Option<String>,
    /// Whether the row renders dimmed.
    pub dim: bool,
}

/// One printed row of the Agents list.
#[derive(Debug, Clone)]
pub enum AgentRow {
    /// The `── functions ──` divider before the first function lane.
    Separator,
    /// A lane header row.
    Lane {
        /// Index into the lanes slice.
        lane_index: usize,
    },
    /// A per-task sublane row under an agent-identity lane.
    Sub {
        /// Index into the lanes slice.
        lane_index: usize,
        /// The task rendered by this sublane.
        task: TaskState,
        /// Whether this is the last shown sublane.
        last: bool,
    },
    /// A `+N more` overflow row when sublanes are capped.
    More {
        /// Index into the lanes slice.
        lane_index: usize,
        /// Number of hidden sublanes.
        hidden: usize,
    },
}

impl AgentRow {
    /// The lane index this row belongs to, if any.
    pub fn lane_index(&self) -> Option<usize> {
        match self {
            AgentRow::Lane { lane_index }
            | AgentRow::Sub { lane_index, .. }
            | AgentRow::More { lane_index, .. } => Some(*lane_index),
            AgentRow::Separator => None,
        }
    }
    /// Whether this row is selectable in the list.
    pub fn selectable(&self) -> bool {
        matches!(self, AgentRow::Lane { .. } | AgentRow::Sub { .. })
    }
}
