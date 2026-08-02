//! Data types for the `chat_store` module.
#[allow(unused_imports)]
use super::*;
/// A conversation turn.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChatMessage {
    /// Message author role.
    pub role: String,
    /// Message text.
    pub content: String,
}
/// An in-memory tree node (messages included on save; loaded from disk on load).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChatNode {
    /// Stable session or thread identifier.
    pub session_id: String,
    /// Human-facing thread name.
    pub name: String,
    /// Parent turn at which this thread forked.
    pub fork_point: Option<i64>,
    /// Materialized messages in this thread.
    pub messages: Vec<ChatMessage>,
    /// Threads forked from this node.
    pub children: Vec<ChatNode>,
}
/// One row for the `/resume` picker — from `tree.json` alone (no md reads).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MainChatSummary {
    /// Stable top-level session identifier.
    pub session_id: String,
    /// Human-facing chat name.
    pub name: String,
    /// Number of turns in the main thread.
    pub turns: usize,
    /// Total threads in the chat tree.
    pub thread_count: usize,
    /// Most recent persisted update timestamp.
    pub updated_at: String,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(super) struct StoredNode {
    #[serde(rename = "sessionId")]
    pub(super) session_id: String,
    pub(super) name: String,
    #[serde(rename = "forkPoint", skip_serializing_if = "Option::is_none", default)]
    pub(super) fork_point: Option<i64>,
    pub(super) turns: usize,
    pub(super) md: String,
    pub(super) children: Vec<StoredNode>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(super) struct StoredTree {
    pub(super) version: u8,
    #[serde(rename = "updatedAt")]
    pub(super) updated_at: String,
    pub(super) root: StoredNode,
}
