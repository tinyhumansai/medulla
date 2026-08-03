//! Folding ACP session updates into Medulla semantic harness events.

use std::time::Instant;

use agent_client_protocol::schema::v1::SessionUpdate;
use serde_json::{json, Value};

use crate::daemon::mappers::{
    pull_request_command, workspace_event_from_output, HarnessSemanticEvent,
};
use crate::protocol::HarnessEvent;
use crate::sessions::WorkspaceContext;

use super::super::types::{OnEvent, OnWorkspaceContext};
use super::types::FoldState;

impl FoldState {
    #[cfg(test)]
    pub(super) fn new(on_event: Option<OnEvent>) -> Self {
        Self::with_workspace(on_event, Default::default(), false, None)
    }

    pub(super) fn with_workspace(
        on_event: Option<OnEvent>,
        workspace_context: WorkspaceContext,
        gh_repo_is_set: bool,
        on_workspace_context: Option<OnWorkspaceContext>,
    ) -> Self {
        Self {
            text: String::new(),
            thought: String::new(),
            events: 0,
            on_event,
            last_activity: Instant::now(),
            tool_calls: Default::default(),
            pull_request_calls: Default::default(),
            workspace_context,
            gh_repo_is_set,
            on_workspace_context,
        }
    }

    /// Fold a standard ACP update into Medulla's existing semantic event model.
    pub(super) fn fold(&mut self, update: SessionUpdate) {
        self.last_activity = Instant::now();
        let value = serde_json::to_value(&update).unwrap_or(Value::Null);
        let kind = value
            .get("sessionUpdate")
            .and_then(Value::as_str)
            .unwrap_or("unknown");
        // Usage telemetry is invisible to the copilot and does not end a
        // reasoning stream. Preserve the cumulative snapshot across it so the
        // next visible thought replaces the row with the complete text.
        if !matches!(kind, "agent_thought_chunk" | "usage_update") {
            self.thought.clear();
        }
        let (event_kind, role, payload) = match kind {
            "agent_message_chunk" => {
                let text = content_text(value.get("content"));
                self.text.push_str(&text);
                ("agent_message", "agent", json!({ "text": text }))
            }
            "agent_thought_chunk" => {
                self.thought.push_str(&content_text(value.get("content")));
                self.thought = crate::daemon::status::redact_reasoning(&self.thought);
                retain_tail(&mut self.thought, 780);
                ("agent_thought", "agent", json!({ "text": self.thought }))
            }
            "tool_call" => ("tool_call", "agent", self.tool_call_payload(&value)),
            "tool_call_update"
                if !matches!(
                    value.get("status").and_then(Value::as_str),
                    Some("completed" | "failed")
                ) =>
            {
                let payload = self.tool_call_payload(&value);
                if value.get("rawInput").is_none() {
                    return;
                }
                ("tool_call", "agent", payload)
            }
            "tool_call_update" => {
                let call_id = value
                    .get("toolCallId")
                    .and_then(Value::as_str)
                    .unwrap_or_default();
                self.tool_calls.remove(call_id);
                self.fold_workspace_result(call_id, value.get("rawOutput"));
                (
                    "tool_result",
                    "tool",
                    json!({
                        "call_id": value.get("toolCallId"),
                        "ok": value.get("status").and_then(Value::as_str) == Some("completed"),
                        "is_error": value.get("status").and_then(Value::as_str) == Some("failed"),
                        "status": value.get("status"),
                        "output": value.get("rawOutput"),
                    }),
                )
            }
            "plan" => ("plan", "agent", value.clone()),
            "usage_update" => ("usage", "system", value.clone()),
            _ => ("status", "system", value.clone()),
        };
        let semantic = HarnessSemanticEvent {
            line: self.events as i64,
            timestamp_ms: now_ms(),
            record_type: format!("acp:{kind}"),
            event: HarnessEvent {
                kind: event_kind.to_string(),
                role: role.to_string(),
                payload,
                ..Default::default()
            },
        };
        self.events += 1;
        if let Some(callback) = self.on_event.as_mut() {
            callback(&semantic);
        }
    }

    pub(super) fn reply(&self) -> String {
        if self.text.trim().is_empty() {
            "ACP agent completed without a text response.".to_string()
        } else {
            self.text.clone()
        }
    }

    /// Merge a partial ACP tool update and expose the complete call to Medulla.
    fn tool_call_payload(&mut self, value: &Value) -> Value {
        let call_id = value
            .get("toolCallId")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        let call = self.tool_calls.entry(call_id.clone()).or_default();
        if let Some(title) = value.get("title").and_then(Value::as_str) {
            call.title = title.to_string();
        }
        if let Some(kind) = value.get("kind").and_then(Value::as_str) {
            call.kind = kind.to_string();
        }
        if let Some(input) = value.get("rawInput") {
            call.input = input.clone();
        }
        if let Some(command) = command_from_input(&call.input) {
            if let Some(operation) = pull_request_command(
                command,
                self.workspace_context.cwd.as_deref(),
                self.gh_repo_is_set,
            ) {
                self.pull_request_calls.insert(call_id.clone(), operation);
            }
        }
        json!({
            "call_id": call_id,
            "tool_name": call.kind,
            "display": call.title,
            "input": call.input,
        })
    }

    /// Correlate a completed ACP terminal call with worktree or PR output.
    fn fold_workspace_result(&mut self, call_id: &str, output: Option<&Value>) {
        let output = output_text(output);
        let operation = self.pull_request_calls.remove(call_id);
        let Some(event) = workspace_event_from_output(
            &output,
            operation,
            self.workspace_context.cwd.as_deref(),
            self.workspace_context.branch.as_deref(),
            self.events as i64,
            now_ms(),
            "acp:workspace",
        ) else {
            return;
        };
        let payload = &event.event.payload;
        let cwd = payload.get("cwd").and_then(Value::as_str);
        let branch = payload.get("branch").and_then(Value::as_str);
        if cwd.is_some_and(|cwd| self.workspace_context.cwd.as_deref() != Some(cwd))
            || branch.is_some_and(|branch| self.workspace_context.branch.as_deref() != Some(branch))
        {
            self.workspace_context.pull_request = None;
        }
        if let Some(cwd) = cwd {
            self.workspace_context.cwd = Some(cwd.to_string());
        }
        if let Some(branch) = branch {
            self.workspace_context.branch = Some(branch.to_string());
        }
        if let Some(pull_request) = payload.get("pull_request").and_then(Value::as_str) {
            self.workspace_context.pull_request = Some(pull_request.to_string());
        }
        if let Some(callback) = self.on_workspace_context.as_ref() {
            callback(self.workspace_context.clone());
        }
        self.events += 1;
        if let Some(callback) = self.on_event.as_mut() {
            callback(&event);
        }
    }
}

fn command_from_input(input: &Value) -> Option<&str> {
    input
        .get("command")
        .or_else(|| input.get("cmd"))
        .and_then(Value::as_str)
}

fn output_text(output: Option<&Value>) -> String {
    match output {
        Some(Value::String(value)) => value.clone(),
        Some(value) => value
            .get("text")
            .and_then(Value::as_str)
            .map(str::to_string)
            .unwrap_or_else(|| value.to_string()),
        None => String::new(),
    }
}

fn content_text(content: Option<&Value>) -> String {
    content
        .and_then(|value| value.get("text"))
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

/// Bound a streamed snapshot while retaining the most recent reasoning.
fn retain_tail(value: &mut String, max_chars: usize) {
    if value.chars().count() <= max_chars {
        return;
    }
    let keep = max_chars.saturating_sub(1);
    let tail = value
        .chars()
        .rev()
        .take(keep)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect::<String>();
    *value = format!("…{tail}");
}

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_millis() as i64)
        .unwrap_or_default()
}
