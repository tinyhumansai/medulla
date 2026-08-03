//! Codex flat-run mapper (`codex exec --json`): fold `event_msg` and
//! `response_item` records into user_prompt, agent_message, agent_thinking,
//! tool_call, tool_result, and status semantic events.
//!
//! Split by responsibility: this file owns the line dispatch and the legacy
//! `payload`-shaped records; [`items`] owns the modern thread/turn item stream.

mod items;

use std::collections::HashMap;

use serde_json::Value;

use items::codex_item;

use super::events::{semantic, tool_call_payload, tool_result_payload, user_prompt_event};
use super::shared::{parse_json_object, parse_maybe_json, text_from_content};
use super::timestamp::parse_timestamp_ms;
use super::types::HarnessSemanticEvent;
use super::work::work_events_for_tool;
use super::workspace::{pull_request_command, workspace_event_from_output, PendingPullRequestCall};

/// Map one raw Codex JSONL `raw` line at `line` into semantic events.
///
/// `pull_request_calls` is mutated as PR commands start and finish, while
/// `workspace_cwd` binds those calls to the checkout active at dispatch time.
/// `gh_repo_is_set` prevents an inherited GitHub repository override from
/// authorizing results for a different checkout.
pub(super) fn codex_events_from_line(
    raw: &str,
    line: i64,
    pull_request_calls: &mut HashMap<String, PendingPullRequestCall>,
    workspace_cwd: Option<&str>,
    workspace_branch: Option<&str>,
    gh_repo_is_set: bool,
) -> Vec<HarnessSemanticEvent> {
    let record = match parse_json_object(raw) {
        Some(record) => record,
        None => return Vec::new(),
    };
    let ts = parse_timestamp_ms(record.get("timestamp"));
    let record_type = record.get("type").and_then(Value::as_str);

    // codex >= ~0.140 `exec --json` is a thread/turn/item event stream, with the
    // detail on a nested `item` rather than the older `payload`. Handle it first;
    // the legacy `event_msg`/`response_item` path follows for older codex.
    match record_type {
        // `thread_id` is captured by the session-id extractor, not as an event.
        Some("thread.started") => return Vec::new(),
        Some("turn.started") => return vec![codex_status(line, ts, "turn.started", "running")],
        Some("turn.completed") | Some("turn.failed") => {
            return vec![codex_status(line, ts, record_type.unwrap(), "idle")]
        }
        Some("item.started") | Some("item.completed") => {
            return codex_item(
                &record,
                record_type.unwrap(),
                ts,
                line,
                pull_request_calls,
                (workspace_cwd, workspace_branch),
                gh_repo_is_set,
            )
        }
        _ => {}
    }

    let payload = match record.get("payload").and_then(Value::as_object) {
        Some(payload) => payload,
        None => return Vec::new(),
    };
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
        let record_type = format!("response_item:{}", payload_type.unwrap());
        if tool_name == "shell" && !call_id.is_empty() {
            if let Some(command) = input
                .get("command")
                .and_then(Value::as_str)
                .and_then(|command| pull_request_command(command, workspace_cwd, gh_repo_is_set))
            {
                pull_request_calls.insert(
                    call_id.to_string(),
                    PendingPullRequestCall::new(command, workspace_cwd, workspace_branch),
                );
            }
        }
        // `update_plan` and `apply_patch` are ordinary function calls on this
        // wire: the plan codex is following and the files it is rewriting are
        // both in here, and both were previously read as anonymous tool use.
        let mut events = vec![semantic(
            line,
            ts,
            &record_type,
            "tool_call",
            "agent",
            tool_call_payload(call_id, tool_name, &input),
        )];
        events.extend(work_events_for_tool(
            line,
            ts,
            &record_type,
            call_id,
            tool_name,
            &input,
        ));
        return events;
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
        let mut events = vec![semantic(
            line,
            ts,
            &format!("response_item:{}", payload_type.unwrap()),
            "tool_result",
            "agent",
            tool_result_payload(call_id, is_error, &output),
        )];
        events.extend(workspace_event_from_output(
            &output,
            pull_request_calls
                .remove(call_id)
                .filter(|_| !is_error)
                .and_then(|call| call.command_in(workspace_cwd, workspace_branch)),
            workspace_cwd,
            workspace_branch,
            line,
            ts,
            &format!("response_item:{}:workspace", payload_type.unwrap()),
        ));
        return events;
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

/// A codex turn-lifecycle status event (`turn.started` → running, `turn.*ed` →
/// idle), mirroring the `task_started`/`task_complete` mapping of older codex.
fn codex_status(line: i64, ts: i64, source: &str, state: &str) -> HarnessSemanticEvent {
    let detail = if state == "running" {
        "working"
    } else {
        "idle"
    };
    semantic(
        line,
        ts,
        &format!("event:{source}"),
        "status",
        "agent",
        serde_json::json!({ "state": state, "detail": detail }),
    )
}

/// Extract the text from a codex tool output (string, or object nesting the text
/// under `content`/`output`/`text`).
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

/// Whether a codex tool output signals an error, checking both the top-level and
/// nested `output` shapes for `success: false` / `is_error: true`.
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
