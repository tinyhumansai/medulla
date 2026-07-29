//! Reading a harness progress frame as the kind of transcript line it is.
//!
//! The copilot reaches a harness through [`crate::flow_engine::caps::dispatch`],
//! whose status channel carries the strings
//! [`crate::daemon::status_detail`] derives — one flat `String` per
//! frame, because that channel is shared with every other dispatch and none of
//! the others want structure.
//!
//! That leaves the interesting half of a turn's progress indistinguishable from
//! the chatter around it: `running workflow_apply_ops: …` arrived looking
//! exactly like `thinking`, so the copilot drew both as dim `·` lines while the
//! orchestrator beside it drew its tool calls as `⏺` with a summary. This module
//! recovers the distinction.
//!
//! The parse is deliberately anchored on [`crate::daemon::TOOL_PREFIX`],
//! the same constant the producer formats with, and
//! [`progress_tests`](mod@self) round-trips a real tool-call event through both
//! so the two cannot drift apart silently.

use crate::daemon::TOOL_PREFIX;

/// What a progress frame turns out to be.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Progress {
    /// A tool call, as `name: display` with the producer's prefix removed.
    Tool(String),
    /// Anything else — thinking, writing, a provider's own wording.
    Status(String),
}

/// Classify one progress frame.
///
/// A frame that says a tool is running becomes [`Progress::Tool`] carrying just
/// the call; everything else passes through as [`Progress::Status`] unchanged.
/// Whitespace-only frames are reported as empty status rather than dropped —
/// the caller's dedup already collapses those, and swallowing them here would
/// hide a provider emitting blanks.
pub fn classify(frame: &str) -> Progress {
    match frame.strip_prefix(TOOL_PREFIX) {
        // A bare prefix with nothing after it is not a tool call worth a line of
        // its own; it says only that something ran.
        Some(rest) if !rest.trim().is_empty() => Progress::Tool(rest.trim().to_string()),
        _ => Progress::Status(frame.to_string()),
    }
}

#[cfg(test)]
#[path = "progress_tests.rs"]
mod progress_tests;
