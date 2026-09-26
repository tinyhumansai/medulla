//! Data types for the `consumer` module.
#[allow(unused_imports)]
use super::*;
/// One tool invocation and (once it lands) its result.
#[derive(Debug, Clone, PartialEq)]
pub struct ToolActivity {
    /// Harness call identifier used to match the result.
    pub call_id: String,
    /// Tool name reported by the harness.
    pub tool_name: String,
    /// Normalized tool family (SDK wire string: `shell|file_read|…|other`).
    pub tool_kind: String,
    /// Human-readable tool action.
    pub display: String,
    /// Stream sequence at which the call started.
    pub started_seq: i64,
    /// Whether a matching result has arrived.
    pub done: bool,
    /// Harness success flag, when reported.
    pub ok: Option<bool>,
    /// Whether the result payload represents an error.
    pub is_error: Option<bool>,
    /// Result size in bytes, when reported.
    pub output_bytes: Option<i64>,
}
/// One entry in the capped human-readable feed.
#[derive(Debug, Clone, PartialEq)]
pub struct FeedEntry {
    /// Source event sequence.
    pub seq: i64,
    /// Source event timestamp.
    pub ts: String,
    /// Chat-bubble side (`owner` for a user prompt, else `agent`).
    pub role: String,
    /// Semantic feed-entry kind.
    pub kind: String,
    /// Human-readable feed text.
    pub text: String,
}
/// Live state for a single agent session.
#[derive(Debug, Clone, PartialEq)]
pub struct SessionView {
    /// The harness provider wire string (`claude`/`codex`/`opencode`), if seen.
    pub provider: Option<String>,
    /// Wrapper-assigned session identifier.
    pub wrapper_session_id: Option<String>,
    /// Underlying harness session identifier.
    pub harness_session_id: Option<String>,
    /// Working directory reported for the session.
    pub cwd: Option<String>,
    /// Derived activity state (SDK `HarnessSessionState` wire string).
    pub status: String,
    /// Current human-readable task or activity.
    pub current_task: String,
    /// Highest folded stream sequence.
    pub last_seq: i64,
    /// Most recently folded event identifier.
    pub last_event_id: Option<String>,
    /// Timestamp of the most recent activity.
    pub last_activity_ts: Option<String>,
    /// Most-recent tool activity, newest last, capped at `limits.tools`.
    pub tools: Vec<ToolActivity>,
    /// Most-recent feed entries, newest last, capped at `limits.feed`.
    pub feed: Vec<FeedEntry>,
    /// What the session is working on: its todo list, plan, sub-agents, and file
    /// edits, folded from the structured work events on the same stream.
    ///
    /// Empty for a harness that reports none of it, which is what a renderer
    /// checks before drawing the surface at all.
    pub work: crate::harness_work::WorkSnapshot,
}
/// Caps for the retained tool and feed histories.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SessionViewLimits {
    /// Maximum retained tool activities.
    pub tools: usize,
    /// Maximum retained feed entries.
    pub feed: usize,
}
