//! Status-line derivation: turn a semantic harness event into the short,
//! human-facing detail string the daemon forwards as a `status` frame. Ported
//! from the TypeScript `statusDetail`.

use crate::tinyplace_support::{HarnessEvent, HarnessEventKind};

/// Derive a short status string from a semantic event (or none). Ported from the
/// TS `statusDetail`.
pub fn status_detail(event: &HarnessEvent) -> Option<String> {
    match event.decoded() {
        HarnessEventKind::ToolCall(payload) => Some(cap(
            &format!("running {}: {}", payload.tool_name, payload.display),
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
