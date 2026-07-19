//! Data model for the JSONL line mappers: the pre-envelope semantic event and
//! the stateful per-stream mapper's fields. The mapper's fold behavior lives in
//! [`mapper`](super::mapper); the fields are `pub(super)` so it can drive them.

use crate::tinyplace::{HarnessEvent, TokenUsage};

/// One typed event parsed from a single transcript line, pre-envelope. Mirrors
/// the TS `HarnessSemanticEvent`; `timestamp_ms` is epoch milliseconds (receive
/// time when the line carries no parseable timestamp).
#[derive(Debug, Clone)]
pub struct HarnessSemanticEvent {
    /// 0-based index of the transcript line this event was folded from.
    pub line: i64,
    /// Epoch milliseconds for the event (receive time when unparseable).
    pub timestamp_ms: i64,
    /// Provider-specific record tag (e.g. `assistant:tool_use`) for diagnostics.
    pub record_type: String,
    /// The shared SDK event this line folded into.
    pub event: HarnessEvent,
}

/// A stateful per-stream line mapper. For codex it also dedupes the
/// double-recorded assistant message; for the other providers it is a plain
/// per-line fold. Create one per run — the dedupe state must not leak across
/// streams.
pub struct HarnessLineMapper {
    /// The provider whose transcript shape this mapper folds.
    pub(super) provider: Provider,
    /// Text of the last agent_message seen (codex dedupe state).
    pub(super) last_text: Option<String>,
    /// Timestamp of the last agent_message seen (codex dedupe window).
    pub(super) last_at_ms: i64,
    /// Latest token usage observed on the stream, if any.
    pub(super) usage: Option<TokenUsage>,
}

/// Which provider's flat-run transcript shape a mapper folds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Provider {
    /// `claude -p --output-format stream-json`.
    Claude,
    /// `codex exec --json`.
    Codex,
    /// `opencode run --format json` (also the unknown-provider fallback).
    Opencode,
}
