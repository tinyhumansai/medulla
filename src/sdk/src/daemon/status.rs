//! Status-line derivation: turn a semantic harness event into the short,
//! human-facing detail string the daemon forwards as a `status` frame. Ported
//! from provider status details, and extended with the work-derived line the
//! newer structured events need.

use crate::harness_work::WorkSnapshot;
use crate::tinyplace::{HarnessEvent, HarnessEventKind};

/// What a tool-call status frame starts with.
///
/// Exported because the wording is a wire format in practice: the copilot reads
/// its own progress channel back to tell a tool call from ordinary chatter
/// ([`crate::ui::workflows::progress`]), and a producer that reworded this
/// without the reader following would silently stop rendering tool calls.
pub const TOOL_PREFIX: &str = "running ";

/// Derive a short status string from a semantic event (or none). Ported from the
/// TS `statusDetail`.
pub fn status_detail(event: &HarnessEvent) -> Option<String> {
    match event.decoded() {
        HarnessEventKind::ToolCall(payload) => Some(cap(
            &format!("{TOOL_PREFIX}{}: {}", payload.tool_name, payload.display),
            200,
        )),
        HarnessEventKind::ToolResult(payload) => Some(
            if payload.is_error {
                "tool failed"
            } else {
                "tool completed"
            }
            .to_string(),
        ),
        HarnessEventKind::AgentThinking(_) => Some("thinking".to_string()),
        HarnessEventKind::AgentMessage(_) => Some("writing response".to_string()),
        HarnessEventKind::Status(payload) => {
            let detail = if payload.detail.is_empty() {
                payload.state
            } else {
                payload.detail
            };
            (!detail.is_empty()).then_some(detail)
        }
        HarnessEventKind::Error(payload) => Some(cap(&format!("error: {}", payload.message), 200)),
        _ => None,
    }
}

/// Truncate `value` to at most `max_chars` characters (char-boundary safe).
fn cap(value: &str, max_chars: usize) -> String {
    if value.chars().count() <= max_chars {
        value.to_string()
    } else {
        value.chars().take(max_chars).collect()
    }
}

/// A status line describing what the harness is working on, for the structured
/// work events that have no wording of their own.
///
/// A `todo_update` or a `subagent_start` decodes to nothing in
/// [`status_detail`] — the published event vocabulary predates them — so
/// without this a harness could rewrite its whole plan and the peer would see
/// no status change at all. Prefers the item the agent says it is on, then the
/// stated goal, then the counts.
pub fn work_detail(work: &WorkSnapshot) -> Option<String> {
    let (done, total) = work.todo_progress();
    let running = work.running_subagents();
    if let Some(current) = work.current_todo() {
        return Some(cap(
            &format!("{} · todo {done}/{total}", current.display()),
            200,
        ));
    }
    if running > 0 {
        let plural = if running == 1 { "" } else { "s" };
        return Some(format!("{running} sub-agent{plural} running"));
    }
    if total > 0 {
        return Some(format!("todo {done}/{total}"));
    }
    work.goal.as_deref().map(|goal| cap(goal, 200))
}
