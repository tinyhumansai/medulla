//! Reading a session file's head window and distilling it into a
//! [`SessionSummary`]: the session id, recorded cwd, and a display label
//! slugged from the first human prompt.
//!
//! Only the first [`HEAD_BYTES`] of each file are read and parsed, which bounds
//! the cost of scanning many transcripts. Claude and Codex use different record
//! shapes, so each has its own head reader.

use std::path::Path;

use serde_json::Value;

use super::types::{SessionAgentKind, SessionSummary};
use crate::ui::util::slug;

/// Bytes read from the head of each transcript when extracting its summary.
pub(super) const HEAD_BYTES: usize = 64 * 1024;

/// Read the head window of `path` and parse it into a [`SessionSummary`],
/// dispatching on the owning agent's record shape. `None` when no session id is
/// present in the head window.
pub(super) fn read_session_summary(agent: SessionAgentKind, path: &Path) -> Option<SessionSummary> {
    let lines = read_head_lines(path);
    match agent {
        SessionAgentKind::Claude => read_claude_summary(&lines),
        SessionAgentKind::Codex => read_codex_summary(&lines),
    }
}

/// Parse Claude head records: `sessionId`/`cwd` fields plus the first `user`
/// message for the label.
pub(super) fn read_claude_summary(lines: &[String]) -> Option<SessionSummary> {
    let mut id: Option<String> = None;
    let mut cwd: Option<String> = None;
    let mut label: Option<String> = None;
    for raw in lines {
        let record = match parse_object(raw) {
            Some(record) => record,
            None => continue,
        };
        if let Some(session_id) = record.get("sessionId").and_then(Value::as_str) {
            id = Some(session_id.to_string());
        }
        if let Some(directory) = record.get("cwd").and_then(Value::as_str) {
            cwd = Some(directory.to_string());
        }
        if label.is_none() && record.get("type").and_then(Value::as_str) == Some("user") {
            label = first_prompt_text(as_message_content(record.get("message")));
        }
    }
    let id = id?;
    Some(SessionSummary {
        id,
        cwd,
        label: label.unwrap_or_else(|| "(no prompt)".to_string()),
    })
}

/// Parse Codex head records: the `session_meta` record's `payload` for
/// id/cwd, and the first user `response_item` message for the label.
pub(super) fn read_codex_summary(lines: &[String]) -> Option<SessionSummary> {
    let mut id: Option<String> = None;
    let mut cwd: Option<String> = None;
    let mut label: Option<String> = None;
    for raw in lines {
        let record = match parse_object(raw) {
            Some(record) => record,
            None => continue,
        };
        if record.get("type").and_then(Value::as_str) == Some("session_meta") {
            if let Some(payload) = record.get("payload").and_then(Value::as_object) {
                if let Some(session_id) = payload
                    .get("session_id")
                    .and_then(Value::as_str)
                    .or_else(|| payload.get("id").and_then(Value::as_str))
                {
                    id = Some(session_id.to_string());
                }
                if let Some(directory) = payload.get("cwd").and_then(Value::as_str) {
                    cwd = Some(directory.to_string());
                }
            }
            continue;
        }
        if label.is_some() || record.get("type").and_then(Value::as_str) != Some("response_item") {
            continue;
        }
        if let Some(payload) = record.get("payload").and_then(Value::as_object) {
            if payload.get("type").and_then(Value::as_str) == Some("message")
                && payload.get("role").and_then(Value::as_str) == Some("user")
            {
                label = first_prompt_text(payload.get("content").cloned());
            }
        }
    }
    let id = id?;
    Some(SessionSummary {
        id,
        cwd,
        label: label.unwrap_or_else(|| "(no prompt)".to_string()),
    })
}

/// Turn a user message's `content` into a display label, or `None` when it is
/// not a real prompt (system-injected `<...>` turns and tool-result turns are
/// skipped so the label reflects the first thing the human said).
pub(super) fn first_prompt_text(content: Option<Value>) -> Option<String> {
    let text = extract_text(content.as_ref())?;
    let trimmed = text.trim();
    if trimmed.is_empty() || trimmed.starts_with('<') {
        return None;
    }
    let label = slug_label(trimmed);
    (!label.is_empty()).then_some(label)
}

/// Pull the plain text out of a message `content`, whether it is a bare string
/// or an array of text blocks (claude `text` / codex `input_text`).
pub(super) fn extract_text(content: Option<&Value>) -> Option<String> {
    match content {
        Some(Value::String(text)) => Some(text.clone()),
        Some(Value::Array(items)) => {
            for item in items {
                let object = match item.as_object() {
                    Some(object) => object,
                    None => continue,
                };
                // claude blocks are {type:"text"}; codex are {type:"input_text"}.
                let kind = object.get("type").and_then(Value::as_str);
                if kind == Some("text") || kind == Some("input_text") {
                    if let Some(text) = object.get("text").and_then(Value::as_str) {
                        return Some(text.to_string());
                    }
                }
            }
            None
        }
        _ => None,
    }
}

/// The `content` of `message` only when it is a `user`-role message, else `None`.
pub(super) fn as_message_content(message: Option<&Value>) -> Option<Value> {
    let object = message?.as_object()?;
    if object.get("role").and_then(Value::as_str) != Some("user") {
        return None;
    }
    object.get("content").cloned()
}

/// Shorten a prompt into a session slug — at most three lowercase words joined
/// by hyphens, e.g. `fix-session-handoff`.
///
/// A whole prompt is far too wide for the row it lands in, and the words that
/// identify the session are almost always its first few. Slugging also handles
/// the safety half: [`slug`] treats every non-alphanumeric byte as a word
/// break, so a pasted escape sequence cannot move the cursor or recolor a pane.
///
/// Returns an empty string for a prompt with no usable word; callers supply
/// their own placeholder.
pub(super) fn slug_label(text: &str) -> String {
    slug(text)
}

/// Read the first [`HEAD_BYTES`] of `path` as UTF-8 (lossy) and split into
/// non-empty lines, dropping a final partial line when the read hit the cap.
fn read_head_lines(path: &Path) -> Vec<String> {
    use std::io::Read;

    let mut file = match std::fs::File::open(path) {
        Ok(file) => file,
        Err(_) => return Vec::new(),
    };
    let mut buffer = vec![0u8; HEAD_BYTES];
    let read = match file.read(&mut buffer) {
        Ok(read) => read,
        Err(_) => return Vec::new(),
    };
    let text = String::from_utf8_lossy(&buffer[..read]);
    let mut lines: Vec<String> = text
        .split('\n')
        .map(|l| l.trim_end_matches('\r').to_string())
        .collect();
    // When the read hit the cap the final line is likely truncated — drop it.
    if read >= HEAD_BYTES && lines.len() > 1 {
        lines.pop();
    }
    lines.into_iter().filter(|line| !line.is_empty()).collect()
}

/// Parse one JSONL line into a JSON object map, or `None` when it is not a
/// well-formed object.
fn parse_object(raw: &str) -> Option<serde_json::Map<String, Value>> {
    let value: Value = serde_json::from_str(raw).ok()?;
    value.as_object().cloned()
}
