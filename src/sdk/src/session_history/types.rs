//! Data model for recent-session history: the agent kind, the public
//! [`RecentSession`] row, and the internal staging types shared across the
//! scanning and summary submodules.
//!
//! These are plain data holders; the behaviour that populates and ranks them
//! lives in the [`list`](super::list), [`scan`](super::scan), and
//! [`summary`](super::summary) submodules.

use std::path::PathBuf;

use serde::Serialize;

/// The coding-agent that owns a session transcript.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum SessionAgentKind {
    /// Anthropic's Claude coding agent (`~/.claude/projects`).
    Claude,
    /// OpenAI's Codex coding agent (`~/.codex/sessions`).
    Codex,
}

impl SessionAgentKind {
    /// The lowercase agent name used as the resume-command binary and in the
    /// dedupe key.
    pub(super) fn as_str(&self) -> &'static str {
        match self {
            SessionAgentKind::Claude => "claude",
            SessionAgentKind::Codex => "codex",
        }
    }
}

/// One recent session, ranked for the resume pane.
#[derive(Debug, Clone, Serialize)]
pub struct RecentSession {
    /// Agent session id (claude `sessionId` / codex `session_id`).
    pub id: String,
    /// The agent that owns this session.
    pub agent: SessionAgentKind,
    /// Working directory the session ran in, when recorded.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cwd: Option<String>,
    /// First human prompt, single-lined + truncated; drives the resume label.
    pub label: String,
    /// Last-activity epoch ms (session-file mtime).
    pub last_active: i64,
    /// Absolute path to the session's JSONL file.
    pub path: String,
}

/// A session transcript file discovered on disk, before its head is parsed.
pub(super) struct RawSessionFile {
    /// The agent that owns the file.
    pub(super) agent: SessionAgentKind,
    /// Absolute path to the JSONL file.
    pub(super) path: PathBuf,
    /// File modification time in epoch ms, used for recency ranking.
    pub(super) mtime_ms: i64,
}

/// The head-window summary of a session file: its id, recorded cwd, and label.
pub(super) struct SessionSummary {
    /// Agent session id parsed from the head records.
    pub(super) id: String,
    /// Working directory recorded in the head window, when present.
    pub(super) cwd: Option<String>,
    /// Display label derived from the first human prompt.
    pub(super) label: String,
}

/// One discovered session file: its canonical path plus the harness session id and
/// cwd read from its head records.
pub(crate) struct DiscoveredSession {
    /// Canonicalized path to the transcript file.
    pub path: PathBuf,
    /// Harness session id parsed from the file.
    pub id: String,
    /// Working directory recorded in the file, when present.
    pub cwd: Option<String>,
}
