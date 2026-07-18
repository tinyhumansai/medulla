//! JSONL line → semantic-event mappers, ported from the tinyplace CLI
//! `harness-events.ts`.
//!
//! Each provider's coding-agent CLI streams a different JSONL transcript shape;
//! these three mappers fold a single raw line into zero or more
//! [`HarnessSemanticEvent`]s over the shared SDK event model
//! ([`HarnessEvent`], re-exported by `tinyplace-proto`). Only the flat run
//! formats are ported here (`claude -p --output-format stream-json`,
//! `codex exec --json`, `opencode run --format json`); the interactive
//! opencode SSE bus mapper belongs to the PTY wrapper, which lands separately.

use serde_json::Value;

use crate::tinyplace_support::HarnessEvent;

/// Truncate cap for tool_result output text (bytes reported separately).
const OUTPUT_CAP: usize = 4096;
/// Truncate cap for raw tool_input carried in tool_call payloads.
const INPUT_CAP: usize = OUTPUT_CAP;
const ELISION: &str = "\n…[truncated]";
/// Codex records each assistant message twice within this window; drop the repeat.
const CODEX_DUPLICATE_WINDOW_MS: i64 = 2000;

/// One typed event parsed from a single transcript line, pre-envelope. Mirrors
/// the TS `HarnessSemanticEvent`; `timestamp_ms` is epoch milliseconds (receive
/// time when the line carries no parseable timestamp).
#[derive(Debug, Clone)]
pub struct HarnessSemanticEvent {
    pub line: i64,
    pub timestamp_ms: i64,
    pub record_type: String,
    pub event: HarnessEvent,
}

/// A stateful per-stream line mapper. For codex it also dedupes the
/// double-recorded assistant message; for the other providers it is a plain
/// per-line fold. Create one per run — the dedupe state must not leak across
/// streams.
pub struct HarnessLineMapper {
    provider: Provider,
    last_text: Option<String>,
    last_at_ms: i64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Provider {
    Claude,
    Codex,
    Opencode,
}

impl HarnessLineMapper {
    /// Build a mapper for `provider` (`"claude" | "codex" | "opencode"`).
    /// Unknown providers yield a mapper that emits nothing.
    pub fn new(provider: &str) -> Self {
        let provider = match provider {
            "claude" => Provider::Claude,
            "codex" => Provider::Codex,
            _ => Provider::Opencode,
        };
        HarnessLineMapper {
            provider,
            last_text: None,
            last_at_ms: i64::MIN,
        }
    }

    /// Map one raw JSONL line into zero or more semantic events.
    pub fn map_line(&mut self, raw: &str, line: i64) -> Vec<HarnessSemanticEvent> {
        let events = match self.provider {
            Provider::Claude => claude_events_from_line(raw, line),
            Provider::Codex => codex_events_from_line(raw, line),
            Provider::Opencode => opencode_events_from_line(raw, line),
        };
        if self.provider != Provider::Codex {
            return events;
        }
        // Codex dedupe: an agent_message whose text equals the previous one
        // within the window is the same message re-recorded, not a new turn.
        events
            .into_iter()
            .filter(|semantic| {
                if semantic.event.kind != "agent_message" {
                    return true;
                }
                let text = semantic
                    .event
                    .payload
                    .get("text")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_string();
                let at_ms = semantic.timestamp_ms;
                let duplicate = self.last_text.as_deref() == Some(text.as_str())
                    && (at_ms - self.last_at_ms).abs() <= CODEX_DUPLICATE_WINDOW_MS;
                self.last_text = Some(text);
                self.last_at_ms = at_ms;
                !duplicate
            })
            .collect()
    }
}

// ── event construction ───────────────────────────────────────────────────────

fn event(kind: &str, role: &str, payload: Value) -> HarnessEvent {
    HarnessEvent {
        kind: kind.to_string(),
        role: role.to_string(),
        payload,
        ..Default::default()
    }
}

fn semantic(
    line: i64,
    timestamp_ms: i64,
    record_type: &str,
    kind: &str,
    role: &str,
    payload: Value,
) -> HarnessSemanticEvent {
    HarnessSemanticEvent {
        line,
        timestamp_ms,
        record_type: record_type.to_string(),
        event: event(kind, role, payload),
    }
}

fn user_prompt_event(line: i64, timestamp_ms: i64, text: &str) -> HarnessSemanticEvent {
    semantic(
        line,
        timestamp_ms,
        "user:prompt",
        "user_prompt",
        "owner",
        serde_json::json!({ "text": text, "source": "human" }),
    )
}

fn tool_result_payload(call_id: &str, is_error: bool, output: &str) -> Value {
    serde_json::json!({
        "call_id": call_id,
        "ok": !is_error,
        "is_error": is_error,
        "output": truncate(output),
        "output_bytes": byte_length(output),
    })
}

fn tool_call_payload(call_id: &str, tool_name: &str, input: &Value) -> Value {
    serde_json::json!({
        "call_id": call_id,
        "tool_name": tool_name,
        "tool_kind": normalize_tool_kind(tool_name),
        "display": tool_display(tool_name, input),
        "input": bound_tool_input(input),
    })
}

// ── Claude ─────────────────────────────────────────────────────────────────

fn claude_events_from_line(raw: &str, line: i64) -> Vec<HarnessSemanticEvent> {
    let record = match parse_json_object(raw) {
        Some(record) => record,
        None => return Vec::new(),
    };
    let ts = parse_timestamp_ms(record.get("timestamp"));
    let message = match record.get("message").and_then(Value::as_object) {
        Some(message) => message,
        None => return Vec::new(),
    };
    let source_role = message.get("role").and_then(Value::as_str);
    let record_type = record.get("type").and_then(Value::as_str);

    if record_type == Some("user") && source_role == Some("user") {
        if let Some(text) = message.get("content").and_then(Value::as_str) {
            return if text.is_empty() {
                Vec::new()
            } else {
                vec![user_prompt_event(line, ts, text)]
            };
        }
        return as_array(message.get("content"))
            .iter()
            .flat_map(|block| claude_user_block(block, line, ts))
            .collect();
    }

    if record_type == Some("assistant") && source_role == Some("assistant") {
        return as_array(message.get("content"))
            .iter()
            .flat_map(|block| claude_assistant_block(block, line, ts))
            .collect();
    }

    Vec::new()
}

fn claude_user_block(block: &Value, line: i64, ts: i64) -> Vec<HarnessSemanticEvent> {
    let object = match block.as_object() {
        Some(object) => object,
        None => return Vec::new(),
    };
    match object.get("type").and_then(Value::as_str) {
        Some("text") => {
            let text = object.get("text").and_then(Value::as_str).unwrap_or("");
            if text.is_empty() {
                Vec::new()
            } else {
                vec![user_prompt_event(line, ts, text)]
            }
        }
        Some("tool_result") => {
            let call_id = object
                .get("tool_use_id")
                .and_then(Value::as_str)
                .unwrap_or("");
            let is_error = object.get("is_error") == Some(&Value::Bool(true));
            let output = flatten_claude_tool_result(object.get("content"));
            vec![semantic(
                line,
                ts,
                "user:tool_result",
                "tool_result",
                "agent",
                tool_result_payload(call_id, is_error, &output),
            )]
        }
        _ => Vec::new(),
    }
}

fn claude_assistant_block(block: &Value, line: i64, ts: i64) -> Vec<HarnessSemanticEvent> {
    let object = match block.as_object() {
        Some(object) => object,
        None => return Vec::new(),
    };
    match object.get("type").and_then(Value::as_str) {
        Some("text") => {
            let text = object.get("text").and_then(Value::as_str).unwrap_or("");
            if text.is_empty() {
                Vec::new()
            } else {
                vec![semantic(
                    line,
                    ts,
                    "assistant:text",
                    "agent_message",
                    "agent",
                    serde_json::json!({ "text": text }),
                )]
            }
        }
        Some("thinking") => {
            let text = object
                .get("thinking")
                .and_then(Value::as_str)
                .or_else(|| object.get("text").and_then(Value::as_str))
                .unwrap_or("");
            if text.is_empty() {
                Vec::new()
            } else {
                vec![semantic(
                    line,
                    ts,
                    "assistant:thinking",
                    "agent_thinking",
                    "agent",
                    serde_json::json!({ "text": text }),
                )]
            }
        }
        Some("tool_use") => {
            let tool_name = object
                .get("name")
                .and_then(Value::as_str)
                .unwrap_or("unknown");
            let call_id = object.get("id").and_then(Value::as_str).unwrap_or("");
            let input = object.get("input").cloned().unwrap_or(Value::Null);
            vec![semantic(
                line,
                ts,
                "assistant:tool_use",
                "tool_call",
                "agent",
                tool_call_payload(call_id, tool_name, &input),
            )]
        }
        _ => Vec::new(),
    }
}

fn flatten_claude_tool_result(content: Option<&Value>) -> String {
    match content {
        Some(Value::String(text)) => text.clone(),
        Some(Value::Array(items)) => items
            .iter()
            .filter_map(|item| {
                let object = item.as_object()?;
                if object.get("type").and_then(Value::as_str) != Some("text") {
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

// ── Codex ──────────────────────────────────────────────────────────────────

fn codex_events_from_line(raw: &str, line: i64) -> Vec<HarnessSemanticEvent> {
    let record = match parse_json_object(raw) {
        Some(record) => record,
        None => return Vec::new(),
    };
    let ts = parse_timestamp_ms(record.get("timestamp"));
    let payload = match record.get("payload").and_then(Value::as_object) {
        Some(payload) => payload,
        None => return Vec::new(),
    };
    let record_type = record.get("type").and_then(Value::as_str);
    let payload_type = payload.get("type").and_then(Value::as_str);

    if record_type == Some("event_msg") && payload_type == Some("user_message") {
        let text = payload.get("message").and_then(Value::as_str).unwrap_or("");
        return if text.is_empty() {
            Vec::new()
        } else {
            vec![user_prompt_event(line, ts, text)]
        };
    }

    let is_agent_message = (record_type == Some("event_msg")
        && payload_type == Some("agent_message"))
        || (record_type == Some("response_item")
            && payload_type == Some("message")
            && payload.get("role").and_then(Value::as_str) == Some("assistant"));
    if is_agent_message {
        let text = payload
            .get("message")
            .and_then(Value::as_str)
            .map(str::to_string)
            .unwrap_or_else(|| text_from_content(payload.get("content"), &["output_text", "text"]));
        return if text.is_empty() {
            Vec::new()
        } else {
            vec![semantic(
                line,
                ts,
                &format!("{}:agent_message", record_type.unwrap_or("record")),
                "agent_message",
                "agent",
                serde_json::json!({ "text": text }),
            )]
        };
    }

    if payload_type == Some("reasoning") {
        let mut text = text_from_content(payload.get("summary"), &["summary_text", "text"]);
        if text.is_empty() {
            text = text_from_content(payload.get("content"), &["reasoning_text", "text"]);
        }
        return if text.is_empty() {
            Vec::new()
        } else {
            vec![semantic(
                line,
                ts,
                "response_item:reasoning",
                "agent_thinking",
                "agent",
                serde_json::json!({ "text": text }),
            )]
        };
    }

    if payload_type == Some("function_call") || payload_type == Some("tool_search_call") {
        let tool_name = payload
            .get("name")
            .and_then(Value::as_str)
            .or_else(|| payload.get("query").and_then(Value::as_str))
            .unwrap_or("tool");
        let input = parse_maybe_json(payload.get("arguments"))
            .or_else(|| payload.get("query").cloned())
            .or_else(|| payload.get("input").cloned())
            .unwrap_or(Value::Null);
        let call_id = payload
            .get("call_id")
            .and_then(Value::as_str)
            .or_else(|| payload.get("id").and_then(Value::as_str))
            .unwrap_or("");
        return vec![semantic(
            line,
            ts,
            &format!("response_item:{}", payload_type.unwrap()),
            "tool_call",
            "agent",
            tool_call_payload(call_id, tool_name, &input),
        )];
    }

    if matches!(
        payload_type,
        Some("function_call_output") | Some("tool_search_output") | Some("mcp_tool_call_end")
    ) {
        let raw_output = payload.get("output").or_else(|| payload.get("result"));
        let output = codex_output_text(raw_output);
        let is_error = codex_is_error(payload);
        let call_id = payload
            .get("call_id")
            .and_then(Value::as_str)
            .or_else(|| payload.get("id").and_then(Value::as_str))
            .unwrap_or("");
        return vec![semantic(
            line,
            ts,
            &format!("response_item:{}", payload_type.unwrap()),
            "tool_result",
            "agent",
            tool_result_payload(call_id, is_error, &output),
        )];
    }

    if payload_type == Some("mcp_tool_call_begin") {
        let tool_name = payload
            .get("tool")
            .and_then(Value::as_str)
            .or_else(|| payload.get("name").and_then(Value::as_str))
            .unwrap_or("mcp");
        let input = parse_maybe_json(payload.get("arguments")).unwrap_or(Value::Null);
        let call_id = payload
            .get("call_id")
            .and_then(Value::as_str)
            .or_else(|| payload.get("id").and_then(Value::as_str))
            .unwrap_or("");
        let mut payload_json = tool_call_payload(call_id, tool_name, &input);
        payload_json["tool_kind"] = Value::String("mcp".to_string());
        return vec![semantic(
            line,
            ts,
            "response_item:mcp_tool_call_begin",
            "tool_call",
            "agent",
            payload_json,
        )];
    }

    if payload_type == Some("task_started") || payload_type == Some("task_complete") {
        let running = payload_type == Some("task_started");
        return vec![semantic(
            line,
            ts,
            &format!("event_msg:{}", payload_type.unwrap()),
            "status",
            "agent",
            serde_json::json!({
                "state": if running { "running" } else { "idle" },
                "detail": if running { "working" } else { "idle" },
            }),
        )];
    }

    Vec::new()
}

fn codex_output_text(output: Option<&Value>) -> String {
    match output {
        Some(Value::String(text)) => text.clone(),
        Some(Value::Object(object)) => {
            let content = object
                .get("content")
                .or_else(|| object.get("output"))
                .or_else(|| object.get("text"));
            match content {
                Some(Value::String(text)) => text.clone(),
                other => text_from_content(other, &["output_text", "text"]),
            }
        }
        other => text_from_content(other, &["output_text", "text"]),
    }
}

fn codex_is_error(payload: &serde_json::Map<String, Value>) -> bool {
    if payload.get("success") == Some(&Value::Bool(false))
        || payload.get("is_error") == Some(&Value::Bool(true))
    {
        return true;
    }
    if let Some(output) = payload.get("output").and_then(Value::as_object) {
        if output.get("success") == Some(&Value::Bool(false))
            || output.get("is_error") == Some(&Value::Bool(true))
        {
            return true;
        }
    }
    false
}

// ── OpenCode (flat `run --format json`) ──────────────────────────────────────

const OPENCODE_TERMINAL_STATES: [&str; 3] = ["completed", "error", "done"];

fn opencode_events_from_line(raw: &str, line: i64) -> Vec<HarnessSemanticEvent> {
    let record = match parse_json_object(raw) {
        Some(record) => record,
        None => return Vec::new(),
    };
    let ts = parse_timestamp_ms(record.get("timestamp"));
    let record_type = record.get("type").and_then(Value::as_str);

    if record_type == Some("error") {
        let message = describe_opencode_error(record.get("error")).unwrap_or_else(|| {
            safe_stringify(
                record
                    .get("error")
                    .unwrap_or(&Value::Object(record.clone())),
            )
        });
        return vec![semantic(
            line,
            ts,
            "opencode:error",
            "error",
            "agent",
            serde_json::json!({ "message": truncate(&message), "fatal": false }),
        )];
    }

    let part = match record.get("part").and_then(Value::as_object) {
        Some(part) => part,
        None => return Vec::new(),
    };

    if record_type == Some("text") {
        if let Some(text) = part.get("text").and_then(Value::as_str) {
            let text = text.trim();
            if !text.is_empty() {
                return vec![semantic(
                    line,
                    ts,
                    "opencode:text",
                    "agent_message",
                    "agent",
                    serde_json::json!({ "text": text }),
                )];
            }
        }
    }

    if record_type == Some("reasoning") {
        if let Some(text) = part.get("text").and_then(Value::as_str) {
            let text = text.trim();
            if !text.is_empty() {
                return vec![semantic(
                    line,
                    ts,
                    "opencode:reasoning",
                    "agent_thinking",
                    "agent",
                    serde_json::json!({ "text": text }),
                )];
            }
        }
    }

    if let Some(tool_name) = part.get("tool").and_then(Value::as_str) {
        if !tool_name.is_empty() {
            let call_id = part.get("callID").and_then(Value::as_str).unwrap_or("");
            let state = part.get("state").and_then(Value::as_object);
            let status = state
                .and_then(|s| s.get("status"))
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_lowercase();
            let output = state.and_then(|s| s.get("output"));
            let terminal = OPENCODE_TERMINAL_STATES.contains(&status.as_str())
                || matches!(output, Some(v) if !v.is_null() && v.as_str() != Some(""));
            if terminal {
                let output_text = open_code_output_text(output);
                let is_error = status == "error";
                return vec![semantic(
                    line,
                    ts,
                    "opencode:tool_result",
                    "tool_result",
                    "agent",
                    tool_result_payload(call_id, is_error, &output_text),
                )];
            }
            let input = state
                .and_then(|s| s.get("input"))
                .cloned()
                .unwrap_or(Value::Null);
            return vec![semantic(
                line,
                ts,
                "opencode:tool_call",
                "tool_call",
                "agent",
                tool_call_payload(call_id, tool_name, &input),
            )];
        }
    }

    Vec::new()
}

fn describe_opencode_error(error: Option<&Value>) -> Option<String> {
    let object = error?.as_object()?;
    let data = object.get("data").and_then(Value::as_object);
    let message = data
        .and_then(|d| d.get("message"))
        .and_then(Value::as_str)
        .filter(|s| !s.trim().is_empty())
        .or_else(|| {
            object
                .get("message")
                .and_then(Value::as_str)
                .filter(|s| !s.trim().is_empty())
        })?;
    let name = object
        .get("name")
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
        .map(|n| format!("{n}: "))
        .unwrap_or_default();
    let reference = data
        .and_then(|d| d.get("ref"))
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
        .map(|r| format!(" ({r})"))
        .unwrap_or_default();
    Some(format!("{name}{message}{reference}"))
}

fn open_code_output_text(output: Option<&Value>) -> String {
    match output {
        Some(Value::String(text)) => text.clone(),
        Some(value) => safe_stringify(value),
        None => String::new(),
    }
}

// ── shared helpers ───────────────────────────────────────────────────────────

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
fn truncate(value: &str) -> String {
    if value.len() <= OUTPUT_CAP {
        return value.to_string();
    }
    let mut end = OUTPUT_CAP;
    while end > 0 && !value.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}{ELISION}", &value[..end])
}

fn byte_length(value: &str) -> i64 {
    value.len() as i64
}

/// Bound the raw tool input before it goes on the wire: keep small structured
/// inputs, collapse oversized ones to a byte-capped serialized string.
fn bound_tool_input(input: &Value) -> Value {
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

fn safe_stringify(value: &Value) -> String {
    match value {
        Value::String(text) => text.clone(),
        other => serde_json::to_string(other).unwrap_or_else(|_| other.to_string()),
    }
}

fn text_from_content(content: Option<&Value>, allowed_types: &[&str]) -> String {
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
fn parse_maybe_json(value: Option<&Value>) -> Option<Value> {
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

fn parse_json_object(raw: &str) -> Option<serde_json::Map<String, Value>> {
    let value: Value = serde_json::from_str(raw).ok()?;
    value.as_object().cloned()
}

fn as_array(value: Option<&Value>) -> Vec<Value> {
    match value {
        Some(Value::Array(items)) => items.clone(),
        _ => Vec::new(),
    }
}

/// Epoch ms for an ISO-8601 string, falling back to receive time. Missing or
/// unparseable timestamps default to *now* (not the Unix epoch), mirroring the
/// TS `parseTimestamp` so the derived status clock never treats a live session
/// as stale.
fn parse_timestamp_ms(value: Option<&Value>) -> i64 {
    match value.and_then(Value::as_str) {
        Some(text) => parse_iso_to_ms(text).unwrap_or_else(now_ms),
        None => now_ms(),
    }
}

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

/// Parse an RFC3339 UTC-ish instant to epoch ms, or `None` if unreadable.
/// Public wrapper over the internal parser so the wrapper's envelope builder can
/// reuse one RFC3339 implementation.
pub fn parse_iso_ms(text: &str) -> Option<i64> {
    parse_iso_to_ms(text)
}

/// Minimal RFC3339 parser (`YYYY-MM-DDTHH:MM:SS(.fff)?(Z|±HH:MM)?`) → epoch ms.
/// Dependency-free; returns `None` for anything it cannot read.
fn parse_iso_to_ms(text: &str) -> Option<i64> {
    let bytes = text.as_bytes();
    if bytes.len() < 19 {
        return None;
    }
    let year: i64 = text.get(0..4)?.parse().ok()?;
    if bytes[4] != b'-' {
        return None;
    }
    let month: i64 = text.get(5..7)?.parse().ok()?;
    if bytes[7] != b'-' {
        return None;
    }
    let day: i64 = text.get(8..10)?.parse().ok()?;
    if bytes[10] != b'T' && bytes[10] != b' ' {
        return None;
    }
    let hour: i64 = text.get(11..13)?.parse().ok()?;
    if bytes[13] != b':' {
        return None;
    }
    let minute: i64 = text.get(14..16)?.parse().ok()?;
    if bytes[16] != b':' {
        return None;
    }
    let second: i64 = text.get(17..19)?.parse().ok()?;

    let mut index = 19;
    let mut millis: i64 = 0;
    if index < bytes.len() && bytes[index] == b'.' {
        index += 1;
        let start = index;
        while index < bytes.len() && bytes[index].is_ascii_digit() {
            index += 1;
        }
        let frac = text.get(start..index)?;
        // Take up to 3 digits for milliseconds; pad if shorter.
        let mut frac_ms = String::new();
        for (position, digit) in frac.chars().enumerate() {
            if position == 3 {
                break;
            }
            frac_ms.push(digit);
        }
        while frac_ms.len() < 3 {
            frac_ms.push('0');
        }
        millis = frac_ms.parse().ok()?;
    }

    // Timezone offset.
    let mut offset_minutes: i64 = 0;
    if index < bytes.len() {
        match bytes[index] {
            b'Z' | b'z' => {}
            b'+' | b'-' => {
                let sign = if bytes[index] == b'-' { -1 } else { 1 };
                let off_h: i64 = text.get(index + 1..index + 3)?.parse().ok()?;
                let off_m: i64 = text.get(index + 4..index + 6)?.parse().ok()?;
                offset_minutes = sign * (off_h * 60 + off_m);
            }
            _ => {}
        }
    }

    let days = days_from_civil(year, month, day);
    let seconds = days * 86_400 + hour * 3600 + minute * 60 + second - offset_minutes * 60;
    Some(seconds * 1000 + millis)
}

/// Days since the Unix epoch for a proleptic-Gregorian date (Howard Hinnant's
/// `days_from_civil`).
fn days_from_civil(year: i64, month: i64, day: i64) -> i64 {
    let y = if month <= 2 { year - 1 } else { year };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;
    let doy = (153 * (if month > 2 { month - 3 } else { month + 9 }) + 2) / 5 + day - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

#[cfg(test)]
mod tests {
    use super::*;

    fn map_all(provider: &str, lines: &[&str]) -> Vec<HarnessSemanticEvent> {
        let mut mapper = HarnessLineMapper::new(provider);
        let mut out = Vec::new();
        for (index, line) in lines.iter().enumerate() {
            out.extend(mapper.map_line(line, index as i64));
        }
        out
    }

    fn kind_of(event: &HarnessSemanticEvent) -> &str {
        &event.event.kind
    }

    #[test]
    fn claude_user_prompt_and_tool_use_and_result() {
        let user = r#"{"type":"user","timestamp":"2026-07-05T00:00:00Z","message":{"role":"user","content":"do the thing"}}"#;
        let assistant = r#"{"type":"assistant","timestamp":"2026-07-05T00:00:01Z","message":{"role":"assistant","content":[{"type":"text","text":"on it"},{"type":"tool_use","id":"t1","name":"Bash","input":{"command":"ls -la"}}]}}"#;
        let result = r#"{"type":"user","timestamp":"2026-07-05T00:00:02Z","message":{"role":"user","content":[{"type":"tool_result","tool_use_id":"t1","is_error":false,"content":"file1\nfile2"}]}}"#;

        let events = map_all("claude", &[user, assistant, result]);
        assert_eq!(kind_of(&events[0]), "user_prompt");
        assert_eq!(events[0].event.payload["text"], "do the thing");
        assert_eq!(events[0].event.role, "owner");
        assert_eq!(kind_of(&events[1]), "agent_message");
        assert_eq!(kind_of(&events[2]), "tool_call");
        assert_eq!(events[2].event.payload["tool_kind"], "shell");
        assert_eq!(events[2].event.payload["display"], "ls -la");
        assert_eq!(events[2].event.payload["call_id"], "t1");
        assert_eq!(kind_of(&events[3]), "tool_result");
        assert_eq!(events[3].event.payload["ok"], true);
        assert_eq!(events[3].event.payload["output"], "file1\nfile2");
        assert_eq!(events[3].event.payload["call_id"], "t1");
    }

    #[test]
    fn codex_dedupes_double_recorded_agent_message() {
        let event_msg = r#"{"type":"event_msg","timestamp":"2026-07-05T00:00:00.000Z","payload":{"type":"agent_message","message":"final answer"}}"#;
        let response_item = r#"{"type":"response_item","timestamp":"2026-07-05T00:00:00.500Z","payload":{"type":"message","role":"assistant","content":[{"type":"output_text","text":"final answer"}]}}"#;
        let events = map_all("codex", &[event_msg, response_item]);
        let messages: Vec<_> = events
            .iter()
            .filter(|e| kind_of(e) == "agent_message")
            .collect();
        assert_eq!(
            messages.len(),
            1,
            "duplicate agent_message should be dropped"
        );
        assert_eq!(messages[0].event.payload["text"], "final answer");
    }

    #[test]
    fn codex_function_call_and_output_and_status() {
        let call = r#"{"type":"response_item","timestamp":"2026-07-05T00:00:00Z","payload":{"type":"function_call","name":"shell","call_id":"c1","arguments":"{\"command\":\"npm test\"}"}}"#;
        let output = r#"{"type":"response_item","timestamp":"2026-07-05T00:00:01Z","payload":{"type":"function_call_output","call_id":"c1","output":"ok","success":true}}"#;
        let started = r#"{"type":"event_msg","timestamp":"2026-07-05T00:00:02Z","payload":{"type":"task_started"}}"#;
        let events = map_all("codex", &[call, output, started]);
        assert_eq!(kind_of(&events[0]), "tool_call");
        assert_eq!(events[0].event.payload["display"], "npm test");
        assert_eq!(events[0].event.payload["tool_kind"], "shell");
        assert_eq!(kind_of(&events[1]), "tool_result");
        assert_eq!(events[1].event.payload["ok"], true);
        assert_eq!(kind_of(&events[2]), "status");
        assert_eq!(events[2].event.payload["state"], "running");
        assert_eq!(events[2].event.payload["detail"], "working");
    }

    #[test]
    fn codex_marks_error_result() {
        let output = r#"{"type":"response_item","timestamp":"2026-07-05T00:00:01Z","payload":{"type":"function_call_output","call_id":"c1","output":"boom","success":false}}"#;
        let events = map_all("codex", &[output]);
        assert_eq!(kind_of(&events[0]), "tool_result");
        assert_eq!(events[0].event.payload["is_error"], true);
        assert_eq!(events[0].event.payload["ok"], false);
    }

    #[test]
    fn opencode_flat_text_tool_and_error() {
        let text = r#"{"type":"text","part":{"type":"text","text":"working on it"}}"#;
        let tool_call = r#"{"type":"tool","part":{"type":"tool","tool":"read","callID":"r1","state":{"status":"running","input":{"file_path":"/a/b.rs"}}}}"#;
        let tool_result = r#"{"type":"tool","part":{"type":"tool","tool":"read","callID":"r1","state":{"status":"completed","output":"contents"}}}"#;
        let error =
            r#"{"type":"error","error":{"name":"ProviderError","data":{"message":"no creds"}}}"#;
        let events = map_all("opencode", &[text, tool_call, tool_result, error]);
        assert_eq!(kind_of(&events[0]), "agent_message");
        assert_eq!(events[0].event.payload["text"], "working on it");
        assert_eq!(kind_of(&events[1]), "tool_call");
        assert_eq!(events[1].event.payload["tool_kind"], "file_read");
        assert_eq!(events[1].event.payload["display"], "/a/b.rs");
        assert_eq!(kind_of(&events[2]), "tool_result");
        assert_eq!(events[2].event.payload["output"], "contents");
        assert_eq!(kind_of(&events[3]), "error");
        assert_eq!(
            events[3].event.payload["message"],
            "ProviderError: no creds"
        );
    }

    #[test]
    fn normalize_tool_kind_ladder() {
        assert_eq!(normalize_tool_kind("mcp__github__list"), "mcp");
        assert_eq!(normalize_tool_kind("Bash"), "shell");
        assert_eq!(normalize_tool_kind("MultiEdit"), "edit");
        assert_eq!(normalize_tool_kind("Write"), "file_write");
        assert_eq!(normalize_tool_kind("Read"), "file_read");
        assert_eq!(normalize_tool_kind("Grep"), "search");
        assert_eq!(normalize_tool_kind("WebFetch"), "web");
        assert_eq!(normalize_tool_kind("Task"), "task");
        assert_eq!(normalize_tool_kind("Xyzzy"), "other");
    }

    #[test]
    fn truncate_caps_on_byte_boundary() {
        let big = "x".repeat(OUTPUT_CAP + 100);
        let out = truncate(&big);
        assert!(out.ends_with(ELISION));
        assert!(out.len() <= OUTPUT_CAP + ELISION.len());
    }

    #[test]
    fn malformed_and_empty_lines_yield_nothing() {
        assert!(map_all("claude", &["not json"]).is_empty());
        assert!(map_all("claude", &["[1,2,3]"]).is_empty()); // not an object
        assert!(map_all("codex", &[r#"{"type":"event_msg"}"#]).is_empty()); // no payload
        assert!(map_all("opencode", &[r#"{"type":"text"}"#]).is_empty()); // no part
                                                                          // An unknown provider maps to opencode semantics but unknown records drop.
        assert!(map_all("mystery", &[r#"{"type":"other","part":{}}"#]).is_empty());
    }

    #[test]
    fn claude_empty_text_and_thinking_fallback() {
        // Empty assistant text produces no event.
        let empty = r#"{"type":"assistant","message":{"role":"assistant","content":[{"type":"text","text":""}]}}"#;
        assert!(map_all("claude", &[empty]).is_empty());

        // A thinking block falling back to the `text` field.
        let thinking = r#"{"type":"assistant","message":{"role":"assistant","content":[{"type":"thinking","text":"pondering"}]}}"#;
        let events = map_all("claude", &[thinking]);
        assert_eq!(kind_of(&events[0]), "agent_thinking");
        assert_eq!(events[0].event.payload["text"], "pondering");

        // A user string prompt (non-array content) and the empty-string case.
        let empty_prompt = r#"{"type":"user","message":{"role":"user","content":""}}"#;
        assert!(map_all("claude", &[empty_prompt]).is_empty());

        // A tool_result with structured array content is flattened + joined.
        let result = r#"{"type":"user","message":{"role":"user","content":[{"type":"tool_result","tool_use_id":"t9","is_error":true,"content":[{"type":"text","text":"line1"},{"type":"text","text":"line2"}]}]}}"#;
        let events = map_all("claude", &[result]);
        assert_eq!(kind_of(&events[0]), "tool_result");
        assert_eq!(events[0].event.payload["is_error"], true);
        assert_eq!(events[0].event.payload["output"], "line1\nline2");
    }

    #[test]
    fn codex_reasoning_mcp_and_search_variants() {
        // Reasoning falling back from summary to content.
        let reasoning = r#"{"type":"response_item","payload":{"type":"reasoning","content":[{"type":"reasoning_text","text":"deep thought"}]}}"#;
        let events = map_all("codex", &[reasoning]);
        assert_eq!(kind_of(&events[0]), "agent_thinking");
        assert_eq!(events[0].event.payload["text"], "deep thought");

        // MCP tool begin overrides tool_kind to "mcp".
        let mcp_begin = r#"{"type":"response_item","payload":{"type":"mcp_tool_call_begin","tool":"lookup","call_id":"m1","arguments":"{\"q\":\"x\"}"}}"#;
        let events = map_all("codex", &[mcp_begin]);
        assert_eq!(kind_of(&events[0]), "tool_call");
        assert_eq!(events[0].event.payload["tool_kind"], "mcp");

        // MCP tool end with a nested error output.
        let mcp_end = r#"{"type":"response_item","payload":{"type":"mcp_tool_call_end","call_id":"m1","output":{"is_error":true,"content":"boom"}}}"#;
        let events = map_all("codex", &[mcp_end]);
        assert_eq!(kind_of(&events[0]), "tool_result");
        assert_eq!(events[0].event.payload["is_error"], true);
        assert_eq!(events[0].event.payload["output"], "boom");

        // A tool_search_call driven off the `query` field.
        let search = r#"{"type":"response_item","payload":{"type":"tool_search_call","query":"ripgrep foo"}}"#;
        let events = map_all("codex", &[search]);
        assert_eq!(kind_of(&events[0]), "tool_call");
        assert_eq!(events[0].event.payload["tool_name"], "ripgrep foo");

        // task_complete → idle status.
        let complete = r#"{"type":"event_msg","payload":{"type":"task_complete"}}"#;
        let events = map_all("codex", &[complete]);
        assert_eq!(kind_of(&events[0]), "status");
        assert_eq!(events[0].event.payload["state"], "idle");
    }

    #[test]
    fn opencode_reasoning_error_ref_and_running_tool() {
        let reasoning = r#"{"type":"reasoning","part":{"type":"reasoning","text":"  thinking  "}}"#;
        let events = map_all("opencode", &[reasoning]);
        assert_eq!(kind_of(&events[0]), "agent_thinking");
        assert_eq!(events[0].event.payload["text"], "thinking");

        // An error with a name + ref suffix.
        let error = r#"{"type":"error","error":{"name":"AuthError","data":{"message":"denied","ref":"E42"}}}"#;
        let events = map_all("opencode", &[error]);
        assert_eq!(
            events[0].event.payload["message"],
            "AuthError: denied (E42)"
        );

        // A running (non-terminal) tool with no output → a tool_call, not result.
        let running = r#"{"type":"tool","part":{"type":"tool","tool":"bash","callID":"b1","state":{"status":"running","input":{"command":"ls"}}}}"#;
        let events = map_all("opencode", &[running]);
        assert_eq!(kind_of(&events[0]), "tool_call");
        assert_eq!(events[0].event.payload["tool_kind"], "shell");
    }

    #[test]
    fn tool_display_and_bound_input_helpers() {
        // Falls back to the tool name when no known key is present.
        assert_eq!(tool_display("Weird", &serde_json::json!({})), "Weird");
        // A bare string input is used directly.
        assert_eq!(
            tool_display("X", &serde_json::json!("just a string")),
            "just a string"
        );
        // Oversized structured input collapses to a byte-capped string.
        let big = "y".repeat(INPUT_CAP + 50);
        let bounded = bound_tool_input(&serde_json::json!({ "blob": big }));
        assert!(bounded.is_string());
        assert!(bounded.as_str().unwrap().ends_with(ELISION));
    }

    #[test]
    fn parse_iso_rejects_garbage() {
        assert!(parse_iso_to_ms("nope").is_none());
        assert!(parse_iso_to_ms("2026/07/05").is_none());
        // A receive-time fallback is used when the field is absent (non-panicking).
        let ts = parse_timestamp_ms(None);
        assert!(ts > 0);
    }

    #[test]
    fn parses_iso_timestamp() {
        // 2026-07-05T00:00:00Z is 1_783_209_600 s since the Unix epoch.
        let ms = parse_iso_to_ms("2026-07-05T00:00:00Z").unwrap();
        assert_eq!(ms, 1_783_209_600_000);
        let with_ms = parse_iso_to_ms("2026-07-05T00:00:00.500Z").unwrap();
        assert_eq!(with_ms, 1_783_209_600_500);
        // Offset handling: 01:00+01:00 is the same instant as 00:00Z.
        let offset = parse_iso_to_ms("2026-07-05T01:00:00+01:00").unwrap();
        assert_eq!(offset, 1_783_209_600_000);
    }
}
