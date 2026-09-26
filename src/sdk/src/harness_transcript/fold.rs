//! Translating one harness semantic event into a transcript line.
//!
//! The single place the wire vocabulary becomes prose. Kept apart from the
//! collector so the phrasing can be unit-tested without a stream, and so a new
//! event kind is one arm here rather than an edit in three modules.

use crate::daemon::mappers::HarnessSemanticEvent;
use crate::protocol::HarnessEventKind;

use super::TranscriptCollector;

impl TranscriptCollector {
    /// Fold one semantic event into the transcript, if it says anything.
    ///
    /// Events that carry no line worth reading back — a status transition, a
    /// lifecycle boundary, the work-snapshot kinds
    /// ([`crate::harness_work::kinds`]) that are rendered from their own fold —
    /// are skipped rather than recorded as blank rows. What is left is the
    /// account of the turn: what the harness said, what it ran, what came back,
    /// and what went wrong.
    pub fn observe(&mut self, event: &HarnessSemanticEvent) {
        let Some((kind, text)) = line_for(event) else {
            return;
        };
        self.push_at(event.timestamp_ms, &kind, &text);
    }
}

/// The `(kind, text)` a transcript records for `event`, or `None` to skip it.
fn line_for(event: &HarnessSemanticEvent) -> Option<(String, String)> {
    let kind = event.event.kind.clone();
    let text = match event.event.decoded() {
        HarnessEventKind::UserPrompt(payload) => payload.text,
        HarnessEventKind::AgentMessage(payload) => payload.text,
        HarnessEventKind::AgentThinking(payload) => payload.text,
        // The display line is what the harness itself shows for the call
        // (`npm test`), and it is the whole reason a tool row is readable. The
        // raw name is the fallback for a harness that supplied no summary —
        // "Bash" alone still says more than a dropped row.
        HarnessEventKind::ToolCall(payload) => {
            let display = payload.display.trim();
            if display.is_empty() {
                payload.tool_name
            } else if payload.tool_name.trim().is_empty() {
                display.to_string()
            } else {
                format!("{}({display})", payload.tool_name)
            }
        }
        // Only failures are recorded. A tool that succeeded already told its
        // story through the call above and the message that follows; keeping
        // every successful result would fill the whole budget with output
        // nobody reads back, and push the failures past the cap.
        HarnessEventKind::ToolResult(payload) => {
            if payload.ok && !payload.is_error {
                return None;
            }
            let output = payload.output.trim();
            match payload.exit_code {
                Some(code) if output.is_empty() => format!("failed (exit {code})"),
                Some(code) => format!("failed (exit {code}): {output}"),
                None if output.is_empty() => "failed".to_string(),
                None => format!("failed: {output}"),
            }
        }
        HarnessEventKind::Error(payload) => payload.message,
        HarnessEventKind::ApprovalRequest(_)
        | HarnessEventKind::Status(_)
        | HarnessEventKind::Lifecycle(_)
        | HarnessEventKind::Unknown(_) => return None,
    };
    Some((kind, text))
}
