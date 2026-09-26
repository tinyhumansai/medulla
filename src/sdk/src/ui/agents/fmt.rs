//! Small formatting helpers shared by the fold: event-kind colours and the
//! header/tool-call string builders. Kept separate so the lane fold in [`super::lanes`]
//! stays focused on the state machine rather than string plumbing.

use crate::ui::events::Usage;

/// Map a task/session event kind onto its display colour, or `None` when the
/// kind has no dedicated colour.
pub(super) fn event_kind_color(kind: &str) -> Option<&'static str> {
    match kind {
        "tool" => Some("blue"),
        "prompt" => Some("cyan"),
        "stdout" => Some("gray"),
        "stderr" | "error" => Some("red"),
        "text" => Some("green"),
        "thinking" => Some("yellow"),
        _ => None,
    }
}

/// Render the `· N↑ M↓` token suffix for a header, or empty when no usage.
pub(super) fn tokens_suffix(usage: &Option<Usage>) -> String {
    match usage {
        Some(u) => format!(" · {}↑ {}↓", u.input_tokens, u.output_tokens),
        None => String::new(),
    }
}

/// Render a single tool call as `→ name(args)`, ellipsizing args past 200 chars.
pub(super) fn tool_line(name: &str, args: &serde_json::Value) -> String {
    let args = serde_json::to_string(args).unwrap_or_else(|_| "{}".into());
    let shown = if args.chars().count() > 200 {
        let mut s: String = args.chars().take(199).collect();
        s.push('…');
        s
    } else {
        args
    };
    format!("→ {name}({shown})")
}
