//! JSON types mirroring the backend API responses.
//!
//! Field names use `serde` renames to match the backend's camelCase wire
//! format exactly. Unknown fields are tolerated so the client keeps working
//! against newer server versions.

use serde::{Deserialize, Serialize};
use serde_json::Value;

// ---------------------------------------------------------------------------
// Auth
// ---------------------------------------------------------------------------

/// Response of `POST /auth/login-token/consume`.
#[derive(Debug, Clone, Deserialize)]
pub struct LoginTokenResult {
    pub jwt: String,
}

/// Audience hint accepted by the login-token consume endpoint.
#[derive(Debug, Clone, Copy, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Audience {
    App,
    Dashboard,
}

// ---------------------------------------------------------------------------
// Sessions (/medulla/v1)
// ---------------------------------------------------------------------------

/// Session lifecycle status.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SessionStatus {
    Active,
    Idle,
    Archived,
    /// Any status not yet modelled by this client.
    #[serde(other)]
    Other,
}

/// Message author role.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    User,
    Assistant,
    #[serde(other)]
    Other,
}

/// Result of creating a session (`POST /medulla/v1/sessions`).
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionCreated {
    pub session_id: String,
}

/// Item in the session list (`GET /medulla/v1/sessions`).
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionSummary {
    pub session_id: String,
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default)]
    pub last_active_at: Option<i64>,
    pub status: SessionStatus,
    #[serde(default)]
    pub last_seq: Option<i64>,
}

/// Detailed session state (`GET /medulla/v1/sessions/:id`).
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionDetail {
    pub session_id: String,
    pub status: SessionStatus,
    #[serde(default)]
    pub last_cycle_id: Option<String>,
    #[serde(default)]
    pub last_seq: Option<i64>,
    #[serde(default)]
    pub event_seq: Option<i64>,
}

/// Result of archiving a session (`DELETE /medulla/v1/sessions/:id`).
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionArchived {
    pub session_id: String,
    pub status: SessionStatus,
}

/// Result of `POST /medulla/v1/sessions/:id/messages`.
///
/// The async (202) response carries `cycle_id`/`seq`; the sync (`?sync=1`)
/// response additionally carries `reply`.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SendResult {
    pub cycle_id: String,
    pub seq: i64,
    #[serde(default)]
    pub reply: Option<String>,
}

/// A replayed message (`GET /medulla/v1/sessions/:id/messages`).
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Message {
    pub seq: i64,
    pub role: Role,
    pub body: String,
    #[serde(default)]
    pub ts: Option<i64>,
    #[serde(default)]
    pub cycle_id: Option<String>,
}

/// Result of `POST /medulla/v1/sessions/:id/abort`.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AbortResult {
    pub session_id: String,
    pub aborted: bool,
}

// ---------------------------------------------------------------------------
// Event stream
// ---------------------------------------------------------------------------

/// Envelope wrapping every event on the session stream.
///
/// `event` retains the raw JSON payload; [`EventEnvelope::kind`] parses it into
/// a typed [`EventKind`].
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EventEnvelope {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub seq: Option<u64>,
    pub at: u64,
    #[serde(rename = "sessionId")]
    pub session_id: String,
    #[serde(rename = "cycleId", default, skip_serializing_if = "Option::is_none")]
    pub cycle_id: Option<String>,
    /// Raw event payload; shape depends on `event.kind`.
    pub event: Value,
}

impl EventEnvelope {
    /// Parse the raw `event` payload into a typed [`EventKind`].
    pub fn kind(&self) -> EventKind {
        EventKind::from_value(&self.event)
    }
}

/// Typed event payload parsed from [`EventEnvelope::event`].
///
/// `Unknown` preserves the raw value for forward-compatibility with event
/// kinds this client does not yet model.
#[derive(Debug, Clone, PartialEq)]
pub enum EventKind {
    /// A user message was recorded.
    User { body: String },
    /// The assistant produced a final message.
    Assistant { body: String },
    /// A cognitive cycle started.
    CycleStart { cycle_id: Option<String> },
    /// A cognitive cycle ended.
    CycleEnd {
        cycle_id: Option<String>,
        pass_count: Option<u64>,
        duration_ms: Option<u64>,
        error: Option<bool>,
    },
    /// An error occurred during a cycle.
    Error { source: String, message: String },
    /// Streaming assistant token delta (unpersisted, no seq).
    AssistantDelta { delta: String },
    /// Streaming reasoning token delta (unpersisted, no seq).
    ReasoningDelta { delta: String },
    /// Streaming tool-call delta (unpersisted); raw payload preserved.
    ToolCallDelta { value: Value },
    /// An event kind not modelled by this client; raw payload preserved.
    Unknown(Value),
}

impl EventKind {
    /// Parse a raw event object (`{ "kind": ..., ... }`) into a typed kind.
    pub fn from_value(v: &Value) -> EventKind {
        let kind = v.get("kind").and_then(Value::as_str).unwrap_or("");
        let str_field = |k: &str| {
            v.get(k)
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string()
        };
        let opt_str = |k: &str| v.get(k).and_then(Value::as_str).map(str::to_string);
        let opt_u64 = |k: &str| v.get(k).and_then(Value::as_u64);
        match kind {
            "user" => EventKind::User {
                body: str_field("body"),
            },
            "assistant" => EventKind::Assistant {
                body: str_field("body"),
            },
            "cycle_start" => EventKind::CycleStart {
                cycle_id: opt_str("cycleId"),
            },
            "cycle_end" => EventKind::CycleEnd {
                cycle_id: opt_str("cycleId"),
                pass_count: opt_u64("passCount"),
                duration_ms: opt_u64("durationMs"),
                error: v.get("error").and_then(Value::as_bool),
            },
            "error" => EventKind::Error {
                source: str_field("source"),
                message: str_field("message"),
            },
            "assistant_delta" => EventKind::AssistantDelta {
                delta: str_field("delta"),
            },
            "reasoning_delta" => EventKind::ReasoningDelta {
                delta: str_field("delta"),
            },
            "tool_call_delta" => EventKind::ToolCallDelta { value: v.clone() },
            _ => EventKind::Unknown(v.clone()),
        }
    }
}

// ---------------------------------------------------------------------------
// Orchestration (/orchestration/v1)
// ---------------------------------------------------------------------------

/// A client-side tool definition offered to a run.
#[derive(Debug, Clone, Serialize)]
pub struct ToolDef {
    pub name: String,
    pub description: String,
    /// JSON-Schema object describing the tool parameters.
    pub parameters: Value,
}

/// A tool call requested by the orchestrator.
#[derive(Debug, Clone, Deserialize)]
pub struct ToolCall {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub args: Value,
}

/// A tool result fed back via `run/continue`.
#[derive(Debug, Clone, Serialize)]
pub struct ToolResult {
    pub id: String,
    pub ok: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// Config knobs for a run (`options.config`).
#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RunConfig {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_passes: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_steps: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_depth: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub context_window_tokens: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub verification: Option<String>,
}

/// Resource limits for a run (`options.limits`).
#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RunLimits {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_concurrency: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_tokens: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub deadline_ms: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_tasks_per_delegate: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_depth: Option<u32>,
}

/// The `options` object of a run request.
#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RunOrchestrationOptions {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub prompt_overrides: Option<std::collections::BTreeMap<String, String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub config: Option<RunConfig>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub limits: Option<RunLimits>,
}

/// Optional inputs to [`crate::client::MedullaClient::run`].
#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RunOptions {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tools: Option<Vec<ToolDef>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub options: Option<RunOrchestrationOptions>,
}

/// Final reply from a tool-less run.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RunReply {
    pub reply: String,
    #[serde(default)]
    pub pass_count: Option<u32>,
    #[serde(default)]
    pub compressed_history: Vec<Value>,
    #[serde(default)]
    pub escalations: Vec<Value>,
    #[serde(default)]
    pub session_id: Option<String>,
    #[serde(default)]
    pub cycle_id: Option<String>,
}

/// A single step of the client tool-loop.
#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "stop", rename_all = "snake_case")]
pub enum LoopEvent {
    /// The orchestrator wants the client to run tools and continue.
    ToolUse {
        #[serde(rename = "cycleId")]
        cycle_id: String,
        #[serde(rename = "sessionId")]
        session_id: String,
        #[serde(rename = "toolCalls", default)]
        tool_calls: Vec<ToolCall>,
    },
    /// The run finished with a final reply.
    End {
        #[serde(rename = "cycleId")]
        cycle_id: String,
        #[serde(rename = "sessionId")]
        session_id: String,
        reply: String,
        #[serde(rename = "passCount", default)]
        pass_count: Option<u32>,
        #[serde(rename = "compressedHistory", default)]
        compressed_history: Vec<Value>,
        #[serde(default)]
        escalations: Vec<Value>,
    },
    /// Long-poll returned without progress; poll `run/continue` again.
    Pending {
        #[serde(rename = "cycleId")]
        cycle_id: String,
        #[serde(rename = "sessionId")]
        session_id: String,
    },
    /// The run errored.
    Error {
        #[serde(rename = "cycleId")]
        cycle_id: String,
        #[serde(rename = "sessionId")]
        session_id: String,
        error: Value,
    },
}

/// Outcome of [`crate::client::MedullaClient::run`]: either a final reply (tool-less)
/// or a tool-loop event (when tools were supplied).
#[derive(Debug, Clone)]
pub enum RunResult {
    Reply(RunReply),
    Loop(LoopEvent),
}
