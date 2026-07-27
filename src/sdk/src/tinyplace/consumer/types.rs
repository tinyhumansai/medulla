//! Data types for the `consumer` module.
#[allow(unused_imports)]
use super::*;
/// One tool invocation and (once it lands) its result.
#[derive(Debug, Clone, PartialEq)]
pub struct ToolActivity {
    pub call_id: String,
    pub tool_name: String,
    /// Normalized tool family (SDK wire string: `shell|file_read|…|other`).
    pub tool_kind: String,
    pub display: String,
    pub started_seq: i64,
    pub done: bool,
    pub ok: Option<bool>,
    pub is_error: Option<bool>,
    pub output_bytes: Option<i64>,
}
/// One entry in the capped human-readable feed.
#[derive(Debug, Clone, PartialEq)]
pub struct FeedEntry {
    pub seq: i64,
    pub ts: String,
    /// Chat-bubble side (`owner` for a user prompt, else `agent`).
    pub role: String,
    pub kind: String,
    pub text: String,
}
/// Live state for a single agent session.
#[derive(Debug, Clone, PartialEq)]
pub struct SessionView {
    /// The harness provider wire string (`claude`/`codex`/`opencode`), if seen.
    pub provider: Option<String>,
    pub wrapper_session_id: Option<String>,
    pub harness_session_id: Option<String>,
    pub cwd: Option<String>,
    /// Derived activity state (SDK `HarnessSessionState` wire string).
    pub status: String,
    pub current_task: String,
    pub last_seq: i64,
    pub last_event_id: Option<String>,
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
    pub tools: usize,
    pub feed: usize,
}
