//! Status-line derivation: turn a semantic harness event into the short,
//! human-facing detail string the daemon forwards as a `status` frame. Ported
//! from provider status details, and extended with the work-derived line the
//! newer structured events need.

use crate::harness_work::WorkSnapshot;
use crate::tinyplace::{HarnessEvent, HarnessEventKind, ToolCallPayload, ToolResultPayload};

/// What a tool-call status frame starts with.
///
/// Exported because the wording is a wire format in practice: the copilot reads
/// its own progress channel back to tell a tool call from ordinary chatter
/// ([`crate::ui::workflows::progress`]), and a producer that reworded this
/// without the reader following would silently stop rendering tool calls.
pub const TOOL_PREFIX: &str = "running ";
/// Prefix for provider-emitted reasoning text forwarded to the copilot.
pub const THINKING_PREFIX: &str = "thinking · ";
/// Internal separator carrying a tool call id through the legacy text channel.
pub(crate) const TOOL_CALL_ID_SEPARATOR: char = '\u{1f}';

/// Derive a short status string from a semantic event (or none). Ported from the
/// TS `statusDetail`.
pub fn status_detail(event: &HarnessEvent) -> Option<String> {
    match event.decoded() {
        HarnessEventKind::ToolCall(payload) => Some(tag_call_id(
            format!("{TOOL_PREFIX}{}", tool_call_detail(&payload)),
            &payload.call_id,
        )),
        HarnessEventKind::ToolResult(payload) => {
            Some(tag_call_id(tool_result_detail(&payload), &payload.call_id))
        }
        HarnessEventKind::AgentThinking(payload) => {
            // Reasoning may echo material the harness just read. Status frames
            // cross the daemon boundary and can be persisted, so scrub the
            // complete text before making it compact.
            let text = safe_reasoning(&payload.text);
            Some(if text.is_empty() {
                "thinking".to_string()
            } else {
                cap(&format!("{THINKING_PREFIX}{text}"), 800)
            })
        }
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

/// Append correlation metadata that the copilot parser removes before display.
fn tag_call_id(detail: String, call_id: &str) -> String {
    if call_id.is_empty() {
        detail
    } else {
        format!("{detail}{TOOL_CALL_ID_SEPARATOR}{call_id}")
    }
}

/// Describe what a tool is actually doing without dumping its input JSON.
fn tool_call_detail(payload: &ToolCallPayload) -> String {
    let input = &payload.input;
    let title = tool_title(payload);
    let detail = input
        .as_str()
        .map(|value| string_input_detail(value, &payload.tool_kind))
        .or_else(|| {
            scalar_at(input, &["command", "cmd", "script"])
                .map(|command| format!("$ {}", safe_command(command)))
        })
        .or_else(|| {
            scalar_at(input, &["file_path", "filePath", "path"])
                .map(|path| one_line(path).to_string())
        })
        .or_else(|| {
            scalar_at(input, &["query", "pattern", "needle"])
                .map(|query| format!("“{}”", one_line(query)))
        })
        .or_else(|| {
            scalar_at(input, &["instruction", "prompt", "task", "objective"])
                .map(|task| one_line(task).to_string())
        })
        .or_else(|| scalar_at(input, &["url", "uri"]).map(safe_url));
    match detail {
        Some(detail) if !detail.is_empty() => cap(&format!("{title} · {detail}"), 180),
        _ if !payload.display.trim().is_empty()
            && !payload.display.trim().eq_ignore_ascii_case(&title) =>
        {
            cap(&format!("{title}: {}", one_line(&payload.display)), 180)
        }
        _ => title,
    }
}

/// Interpret bare-string tool input without bypassing credential redaction.
fn string_input_detail(value: &str, tool_kind: &str) -> String {
    if tool_kind == "shell" {
        format!("$ {}", safe_command(value))
    } else if value.contains("://") {
        safe_url(value)
    } else if credential_shaped(value) {
        "[credential redacted]".to_string()
    } else {
        one_line(value)
    }
}

/// Prefer a human surface name, falling back to the protocol tool name/kind.
fn tool_title(payload: &ToolCallPayload) -> String {
    let display = payload.display.trim();
    let name = payload.tool_name.trim();
    match (name, payload.tool_kind.as_str()) {
        ("execute", _) | ("", "shell") => "Terminal".to_string(),
        ("", _) if scalar_at(&payload.input, &["command", "cmd", "script"]).is_some() => {
            "Terminal".to_string()
        }
        ("read", _) | (_, "file_read") => "Read".to_string(),
        ("write", _) | (_, "file_write") => "Write".to_string(),
        ("edit", _) | (_, "edit") => "Edit".to_string(),
        ("search", _) | (_, "search") => "Search".to_string(),
        ("", _) if !display.is_empty() => display.to_string(),
        ("", kind) if !kind.is_empty() => title_case(kind),
        ("", _) => "Tool".to_string(),
        (name, _) => title_case(name),
    }
}

/// Describe settlement without exposing tool output, which may contain secrets.
fn tool_result_detail(payload: &ToolResultPayload) -> String {
    let failed = payload.is_error || payload.exit_code.is_some_and(|code| code != 0);
    let title = if failed {
        "tool failed"
    } else {
        "tool completed"
    };
    if let Some(code) = payload.exit_code {
        return format!("{title} · exit {code}");
    }
    if payload.output_bytes > 0 {
        return format!(
            "{title} · {} output",
            byte_count(payload.output_bytes as u64)
        );
    }
    title.to_string()
}

/// Keep useful command text unless it carries credential-shaped material.
///
/// Status lines cross the daemon boundary and can be persisted by peers, so a
/// false positive costs only detail while a false negative leaks a credential.
fn safe_command(command: &str) -> String {
    let command = one_line(command);
    if credential_shaped(&command)
        || command
            .split_whitespace()
            .any(|part| part.contains("://") && (part.contains('?') || has_url_userinfo(part)))
    {
        "[credential redacted]".to_string()
    } else {
        command
    }
}

/// Scrub reasoning before it crosses the daemon boundary.
///
/// The history scrubber handles known token formats and assignments. This
/// second pass covers credential-bearing URLs and authorization forms that are
/// unsafe even when their secret does not match a known token shape.
fn safe_reasoning(reasoning: &str) -> String {
    one_line(&redact_reasoning(reasoning))
}

/// Redact reasoning while preserving its whitespace for streamed accumulation.
///
/// Providers must call this before bounding a cumulative snapshot: truncating
/// raw text first can remove the prefix that makes a credential detectable.
pub(crate) fn redact_reasoning(reasoning: &str) -> String {
    let (redacted, _) = crate::history_upload::redact_text(reasoning);
    if credential_shaped(&redacted)
        || redacted.split_whitespace().any(|part| {
            part.contains("://") && (has_url_userinfo(part) || part.contains(['?', '#']))
        })
    {
        "[credential redacted]".to_string()
    } else {
        redacted
    }
}

/// Remove URL components commonly used to carry credentials.
fn safe_url(url: &str) -> String {
    let url = one_line(url);
    if credential_shaped(&url) || has_url_userinfo(&url) {
        return "[credential redacted URL]".to_string();
    }
    match url.find(['?', '#']) {
        Some(index) => format!("{}?••••", &url[..index]),
        None => url,
    }
}

/// Detect names and schemes that commonly introduce inline credentials.
fn credential_shaped(value: &str) -> bool {
    let lower = value.to_ascii_lowercase();
    [
        "authorization",
        "bearer ",
        "token",
        "secret",
        "password",
        "passwd",
        "api_key",
        "api-key",
        "apikey",
        "access_key",
        "private_key",
    ]
    .iter()
    .any(|needle| lower.contains(needle))
}

/// Return whether a URL authority contains `user:password@` style userinfo.
fn has_url_userinfo(value: &str) -> bool {
    let Some(authority) = value.split_once("://").map(|(_, rest)| rest) else {
        return false;
    };
    let authority = authority.split(['/', '?', '#']).next().unwrap_or(authority);
    authority.contains('@')
}

/// Read the first string or scalar at a known-safe key.
fn scalar_at<'a>(value: &'a serde_json::Value, keys: &[&str]) -> Option<&'a str> {
    let object = value.as_object()?;
    keys.iter().find_map(|key| object.get(*key)?.as_str())
}

/// Collapse a potentially multiline value before it enters a one-line status.
fn one_line(value: &str) -> String {
    value.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Turn a protocol identifier into a compact title.
fn title_case(value: &str) -> String {
    value
        .split(['_', '-', ':'])
        .filter(|part| !part.is_empty())
        .map(|part| {
            let mut chars = part.chars();
            match chars.next() {
                Some(first) => format!("{}{}", first.to_uppercase(), chars.as_str()),
                None => String::new(),
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// Humanize a byte count without false precision.
fn byte_count(bytes: u64) -> String {
    if bytes < 1024 {
        format!("{bytes} B")
    } else {
        format!("{:.1} KiB", bytes as f64 / 1024.0)
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
