//! ACP-backed harness execution.
//!
//! This module is the provider-neutral process boundary for daemon tasks. Claude
//! Code and Codex are reached through their official ACP adapters; OpenCode
//! exposes ACP directly. Everything above this module continues to consume the
//! stable Medulla semantic-event surface.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use agent_client_protocol::schema::v1::{
    CancelNotification, ContentBlock, Error as AcpError, InitializeRequest, LoadSessionRequest,
    NewSessionRequest, PromptRequest, RequestPermissionOutcome, RequestPermissionRequest,
    RequestPermissionResponse, SelectedPermissionOutcome, SessionNotification, SessionUpdate,
};
use agent_client_protocol::schema::ProtocolVersion;
use agent_client_protocol::{AcpAgent, AcpAgentConfig, Agent, ConnectionTo};
use serde_json::{json, Value};

use crate::daemon::mappers::HarnessSemanticEvent;
use crate::tinyplace::{HarnessEvent, HarnessProvider};

use super::types::{OnEvent, RunTaskOptions, RunTaskResult};
pub(super) use types::FoldState;

mod types;

#[cfg(test)]
mod tests;

/// The MCP servers offered to every ACP session.
///
/// Just the workflow tools, served by this same binary in a subprocess. Empty
/// when the binary's path cannot be determined — a session without the tools is
/// a session that can still do its actual job, which is better than failing to
/// start one.
fn workflow_mcp_servers(
    tool_mode: Option<&str>,
) -> Vec<agent_client_protocol::schema::v1::McpServer> {
    #[cfg(not(feature = "workflows"))]
    {
        let _ = tool_mode;
        Vec::new()
    }
    #[cfg(feature = "workflows")]
    {
        use agent_client_protocol::schema::v1::{McpServer, McpServerStdio};
        // An operator who turned workflows off should not have harnesses handed
        // tools that would be refused.
        let env: std::collections::HashMap<String, String> = std::env::vars().collect();
        let cwd = std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."));
        // Respects the parent's `--config`, if one was recorded — see
        // `crate::config::CONFIG_PATH_ENV`. Rediscovering here regardless of
        // that would let a policy the operator explicitly chose (say,
        // `allowCode = false`) be silently overridden by whatever config this
        // subprocess's own `cwd` happens to discover.
        let enabled =
            crate::config::load_config(crate::config::explicit_config_from_env(&env), &env, &cwd)
                .map(|loaded| loaded.config.workflows.enabled)
                .unwrap_or(true);
        if !enabled {
            return Vec::new();
        }
        let Ok(binary) = std::env::current_exe() else {
            return Vec::new();
        };
        // The subprocess inherits this process's environment, which is what
        // carries MEDULLA_HOME — so the harness edits the same workflow store
        // the operator sees.
        //
        // The tool mode is passed *explicitly* rather than inherited, because
        // it is the one setting that differs per task: this daemon serves
        // authoring turns and review turns from one process, so an inherited
        // value could only ever be right for one of them. A review turn that
        // silently got the full surface could rewrite the graph it was asked
        // only to review, which is exactly what the mode exists to prevent.
        let mut server = McpServerStdio::new(crate::workflows::mcp::SERVER_NAME, binary)
            .args(vec!["workflow".to_string(), "mcp".to_string()]);
        if let Some(mode) = tool_mode {
            let (mode, scope) = mode
                .split_once(':')
                .map_or((mode, None), |(mode, scope)| (mode, Some(scope)));
            server
                .env
                .push(agent_client_protocol::schema::v1::EnvVariable::new(
                    crate::workflows::mcp::TOOL_MODE_ENV,
                    mode,
                ));
            if let Some(scope) = scope {
                server
                    .env
                    .push(agent_client_protocol::schema::v1::EnvVariable::new(
                        crate::workflows::mcp::TOOL_SCOPE_ENV,
                        scope,
                    ));
            }
        }
        vec![McpServer::Stdio(server)]
    }
}

/// Environment switch selecting ACP instead of legacy provider JSONL.
pub const HARNESS_PROTOCOL_ENV: &str = "MEDULLA_HARNESS_PROTOCOL";

/// Whether this task explicitly requests the ACP transport.
pub(super) fn uses_acp(options: &RunTaskOptions) -> bool {
    options
        .env
        .get(HARNESS_PROTOCOL_ENV)
        .is_some_and(|value| value.eq_ignore_ascii_case("acp"))
}

/// Execute one task through the standard Agent Client Protocol.
pub async fn run_acp_task(options: RunTaskOptions) -> Result<RunTaskResult, String> {
    let agent = agent_for(&options);
    // Read before `options` is picked apart below, and cloned because the
    // session setup runs inside an async move closure.
    #[cfg(feature = "workflows")]
    let tool_mode: Option<String> = options
        .env
        .get(crate::workflows::mcp::TOOL_MODE_ENV)
        .cloned();
    #[cfg(not(feature = "workflows"))]
    let tool_mode: Option<String> = None;
    let state = Arc::new(Mutex::new(FoldState::new(options.on_event)));
    let notification_state = state.clone();
    let approve = options.skip_permissions;
    let cwd = PathBuf::from(&options.cwd);
    let resume = options.resume_session_id.clone();
    let prompt = options.prompt.clone();
    let abort = options.abort.clone();
    let provider = options.provider;
    let timeout = Duration::from_millis(options.timeout_ms);

    agent_client_protocol::Client
        .builder()
        .on_receive_notification(
            async move |notification: SessionNotification, _cx| {
                notification_state.lock().unwrap().fold(notification.update);
                Ok(())
            },
            agent_client_protocol::on_receive_notification!(),
        )
        .on_receive_request(
            async move |request: RequestPermissionRequest, responder, _cx| {
                let outcome = if approve {
                    request
                        .options
                        .first()
                        .map(|option| {
                            RequestPermissionOutcome::Selected(SelectedPermissionOutcome::new(
                                option.option_id.clone(),
                            ))
                        })
                        .unwrap_or(RequestPermissionOutcome::Cancelled)
                } else {
                    RequestPermissionOutcome::Cancelled
                };
                responder.respond(RequestPermissionResponse::new(outcome))
            },
            agent_client_protocol::on_receive_request!(),
        )
        .connect_with(agent, |connection: ConnectionTo<Agent>| async move {
            connection
                .send_request(InitializeRequest::new(ProtocolVersion::V1))
                .block_task()
                .await?;

            let session_id = match resume {
                Some(id) => {
                    let mut request = LoadSessionRequest::new(id.clone(), cwd.clone());
                    // The same servers a new session gets, and for the same
                    // reason. A loaded session restores the *transcript*, not
                    // the client's tool offer — so omitting this hands the
                    // harness a conversation it remembers and no way to act on
                    // it. For the copilot that is the difference between an
                    // agent that edits the graph and one that can only discuss
                    // it, and it would fail silently: the model would explain
                    // what it would have done.
                    request.mcp_servers = workflow_mcp_servers(tool_mode.as_deref());
                    connection.send_request(request).block_task().await?;
                    id.into()
                }
                None => {
                    let mut request = NewSessionRequest::new(cwd);
                    // Offer the harness Medulla's own tools. Medulla is the ACP
                    // *client* here, so this is the only way it can hand a
                    // harness anything to call — and it is what lets a coding
                    // agent author a workflow rather than only run one.
                    request.mcp_servers = workflow_mcp_servers(tool_mode.as_deref());
                    connection
                        .send_request(request)
                        .block_task()
                        .await?
                        .session_id
                }
            };

            let request = connection
                .send_request(PromptRequest::new(
                    session_id.clone(),
                    vec![ContentBlock::from(prompt)],
                ))
                .block_task();
            let idle_state = state.clone();
            let idle = async move {
                loop {
                    let deadline = idle_state.lock().unwrap().last_activity + timeout;
                    tokio::time::sleep_until(deadline.into()).await;
                    if Instant::now().duration_since(idle_state.lock().unwrap().last_activity)
                        >= timeout
                    {
                        break;
                    }
                }
            };
            tokio::pin!(request);
            tokio::pin!(idle);
            tokio::select! {
                result = &mut request => {
                    result?;
                }
                _ = abort.cancelled() => {
                    connection
                        .send_notification(CancelNotification::new(session_id.clone()))
                        ?;
                    return Err(AcpError::request_cancelled());
                }
                _ = &mut idle => {
                    connection.send_notification(CancelNotification::new(session_id.clone()))?;
                    return Err(AcpError::new(
                        -32603,
                        format!("ACP task idle for {}ms (no updates)", timeout.as_millis()),
                    ));
                }
            }

            let state = state.lock().unwrap();
            Ok(RunTaskResult {
                provider,
                reply: state.reply(),
                events: state.events,
                usage: None,
                session_id: Some(session_id.to_string()),
            })
        })
        .await
        .map_err(|error| format!("{} ACP error: {error}", provider.as_str()))
}

/// Construct the ACP server command for a supported harness.
fn agent_for(options: &RunTaskOptions) -> AcpAgent {
    let config = match options.provider {
        HarnessProvider::Claude => {
            AcpAgentConfig::new("npx").args(["-y", "@agentclientprotocol/claude-agent-acp@latest"])
        }
        HarnessProvider::Codex => {
            AcpAgentConfig::new("npx").args(["-y", "@agentclientprotocol/codex-acp@latest"])
        }
        HarnessProvider::Opencode => AcpAgentConfig::new(crate::tinyplace::env::provider_bin(
            HarnessProvider::Opencode,
            &options.env,
        ))
        .arg("acp"),
    };
    AcpAgent::new(config.envs(acp_env(options)))
}

/// The environment handed to the ACP agent process.
///
/// The agent process is the one that ends up running `git commit`, so the
/// attribution hook env has to land here too: `run_provider_task` dispatches to
/// ACP *before* the spawn seam in `execute` that handles it for direct runs.
pub(super) fn acp_env(options: &RunTaskOptions) -> HashMap<String, String> {
    let mut env = options.env.clone();
    env.extend(crate::attribution::attribution_env(options.attribution));
    env
}

impl FoldState {
    pub(super) fn new(on_event: Option<OnEvent>) -> Self {
        Self {
            text: String::new(),
            thought: String::new(),
            events: 0,
            on_event,
            last_activity: Instant::now(),
            tool_calls: Default::default(),
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
        if kind != "agent_thought_chunk" {
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
        json!({
            "call_id": call_id,
            "tool_name": call.kind,
            "display": call.title,
            "input": call.input,
        })
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
