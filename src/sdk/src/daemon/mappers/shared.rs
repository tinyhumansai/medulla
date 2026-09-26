//! Provider-agnostic text and tool helpers plus the truncation caps: tool-name
//! normalization, one-line call summaries, byte-capped truncation, structured
//! input bounding, and the small JSON-shape utilities the three provider mappers
//! share.

use serde_json::Value;

/// Truncate cap for tool_result output text (bytes reported separately).
pub(super) const OUTPUT_CAP: usize = 4096;
/// Truncate cap for raw tool_input carried in tool_call payloads.
pub(super) const INPUT_CAP: usize = OUTPUT_CAP;
/// Marker appended when a value is cut to fit a cap.
pub(super) const ELISION: &str = "\n…[truncated]";

/// Normalize a raw tool name to a coarse tool family. Ported ladder from
/// `normalizeToolKind`.
pub fn normalize_tool_kind(name: &str) -> &'static str {
    let lower = name.to_lowercase();
    if lower.starts_with("mcp__") || lower.contains("mcp") {
        return "mcp";
    }
    if contains_any(
        &lower,
        &["bash", "shell", "exec", "command", "terminal", "run"],
    ) {
        return "shell";
    }
    if contains_any(&lower, &["multiedit", "edit", "apply_patch", "patch"]) {
        return "edit";
    }
    if contains_any(&lower, &["write", "create_file"]) {
        return "file_write";
    }
    if contains_any(&lower, &["read", "cat", "open_file", "view"]) {
        return "file_read";
    }
    if contains_any(&lower, &["grep", "glob", "search", "find", "ripgrep"]) {
        return "search";
    }
    if contains_any(&lower, &["web", "fetch", "http", "browse", "url"]) {
        return "web";
    }
    if contains_any(&lower, &["task", "agent", "subagent", "spawn"]) {
        return "task";
    }
    "other"
}

/// True when `haystack` contains any of the `needles`.
fn contains_any(haystack: &str, needles: &[&str]) -> bool {
    needles.iter().any(|needle| haystack.contains(needle))
}

/// Best-effort one-line summary of what a tool call is doing.
pub fn tool_display(name: &str, input: &Value) -> String {
    if let Some(object) = input.as_object() {
        for key in [
            "command",
            "cmd",
            "file_path",
            "path",
            "pattern",
            "query",
            "url",
            "prompt",
            "description",
        ] {
            if let Some(value) = object.get(key).and_then(Value::as_str) {
                if !value.is_empty() {
                    return first_line(value);
                }
            }
        }
    }
    if let Some(text) = input.as_str() {
        if !text.is_empty() {
            return first_line(text);
        }
    }
    name.to_string()
}

/// First line of `value`, capped at 200 chars with an ellipsis when longer.
fn first_line(value: &str) -> String {
    let line = value.split('\n').next().unwrap_or(value);
    if line.chars().count() > 200 {
        let prefix: String = line.chars().take(197).collect();
        format!("{prefix}...")
    } else {
        line.to_string()
    }
}

/// Byte-cap a string to [`OUTPUT_CAP`], appending the elision marker when cut.
/// Slices on a UTF-8 boundary so a multi-byte payload never splits a code point.
pub(super) fn truncate(value: &str) -> String {
    if value.len() <= OUTPUT_CAP {
        return value.to_string();
    }
    let mut end = OUTPUT_CAP;
    while end > 0 && !value.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}{ELISION}", &value[..end])
}

/// Byte length of a string as `i64` (reported alongside truncated output).
pub(super) fn byte_length(value: &str) -> i64 {
    value.len() as i64
}

/// Bound the raw tool input before it goes on the wire: keep small structured
/// inputs, collapse oversized ones to a byte-capped serialized string.
pub(super) fn bound_tool_input(input: &Value) -> Value {
    let serialized = safe_stringify(input);
    if serialized.len() <= INPUT_CAP {
        return input.clone();
    }
    let mut end = INPUT_CAP;
    while end > 0 && !serialized.is_char_boundary(end) {
        end -= 1;
    }
    Value::String(format!("{}{ELISION}", &serialized[..end]))
}

/// Render a JSON value as a string, using the string content directly and
/// falling back to compact JSON serialization for structured values.
pub(super) fn safe_stringify(value: &Value) -> String {
    match value {
        Value::String(text) => text.clone(),
        other => serde_json::to_string(other).unwrap_or_else(|_| other.to_string()),
    }
}

/// Flatten `content` (a string or array of typed blocks) into joined text,
/// keeping only blocks whose `type` is in `allowed_types`.
pub(super) fn text_from_content(content: Option<&Value>, allowed_types: &[&str]) -> String {
    match content {
        Some(Value::String(text)) => text.clone(),
        Some(Value::Array(items)) => items
            .iter()
            .filter_map(|item| {
                let object = item.as_object()?;
                let kind = object.get("type").and_then(Value::as_str).unwrap_or("");
                if !allowed_types.contains(&kind) {
                    return None;
                }
                object
                    .get("text")
                    .and_then(Value::as_str)
                    .map(str::to_string)
            })
            .collect::<Vec<_>>()
            .join("\n"),
        _ => String::new(),
    }
}

/// Codex serializes call arguments as a JSON string; parse it back to structured
/// data. Returns `None` when the input is absent, `Some(original)` when it is not
/// a JSON-object/array string.
pub(super) fn parse_maybe_json(value: Option<&Value>) -> Option<Value> {
    let value = value?;
    let Value::String(text) = value else {
        return Some(value.clone());
    };
    let trimmed = text.trim();
    if !trimmed.starts_with('{') && !trimmed.starts_with('[') {
        return Some(value.clone());
    }
    match serde_json::from_str::<Value>(trimmed) {
        Ok(parsed) => Some(parsed),
        Err(_) => Some(value.clone()),
    }
}

/// Parse a raw JSONL line into a JSON object map, or `None` if it is not an
/// object (or not valid JSON).
pub(super) fn parse_json_object(raw: &str) -> Option<serde_json::Map<String, Value>> {
    let value: Value = serde_json::from_str(raw).ok()?;
    value.as_object().cloned()
}

/// The array items of `value`, or an empty vector for any non-array value.
pub(super) fn as_array(value: Option<&Value>) -> Vec<Value> {
    match value {
        Some(Value::Array(items)) => items.clone(),
        _ => Vec::new(),
    }
}
