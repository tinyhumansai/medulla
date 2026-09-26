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
    /// Bearer token issued for subsequent authenticated requests.
    pub jwt: String,
}

/// Audience hint accepted by the login-token consume endpoint.
#[derive(Debug, Clone, Copy, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Audience {
    /// Native or terminal application login.
    App,
    /// Browser dashboard login.
    Dashboard,
}

// ---------------------------------------------------------------------------
// Sessions (/medulla/v1)
// ---------------------------------------------------------------------------

/// Session lifecycle status.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SessionStatus {
    /// Session is currently processing or ready for input.
    Active,
    /// Session exists but has no active cycle.
    Idle,
    /// Session was archived and is no longer active.
    Archived,
    /// Any status not yet modelled by this client.
    #[serde(other)]
    Other,
}

/// Message author role.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    /// Message authored by the user.
    User,
    /// Message authored by the assistant.
    Assistant,
    /// A forward-compatible role not yet modelled by the SDK.
    #[serde(other)]
    Other,
}

/// Result of creating a session (`POST /medulla/v1/sessions`).
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionCreated {
    /// Identifier assigned to the new session.
    pub session_id: String,
}

/// Item in the session list (`GET /medulla/v1/sessions`).
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionSummary {
    /// Stable session identifier.
    pub session_id: String,
    /// Optional human-facing session title.
    #[serde(default)]
    pub title: Option<String>,
    /// Unix timestamp of the most recent activity.
    #[serde(default)]
    pub last_active_at: Option<i64>,
    /// Current lifecycle state.
    pub status: SessionStatus,
    /// Most recent persisted event sequence.
    #[serde(default)]
    pub last_seq: Option<i64>,
}

/// Detailed session state (`GET /medulla/v1/sessions/:id`).
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionDetail {
    /// Stable session identifier.
    pub session_id: String,
    /// Current lifecycle state.
    pub status: SessionStatus,
    /// Identifier of the most recently started cycle.
    #[serde(default)]
    pub last_cycle_id: Option<String>,
    /// Most recent persisted message sequence.
    #[serde(default)]
    pub last_seq: Option<i64>,
    /// Most recent event-stream sequence.
    #[serde(default)]
    pub event_seq: Option<i64>,
}

/// Result of archiving a session (`DELETE /medulla/v1/sessions/:id`).
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionArchived {
    /// Identifier of the archived session.
    pub session_id: String,
    /// Resulting lifecycle state.
    pub status: SessionStatus,
}

/// Result of `POST /medulla/v1/sessions/:id/messages`.
///
/// The async (202) response carries `cycle_id`/`seq`; the sync (`?sync=1`)
/// response additionally carries `reply`.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SendResult {
    /// Cognitive cycle created for the message.
    pub cycle_id: String,
    /// Persisted message sequence.
    pub seq: i64,
    /// Final assistant response when synchronous mode was requested.
    #[serde(default)]
    pub reply: Option<String>,
}

/// A replayed message (`GET /medulla/v1/sessions/:id/messages`).
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Message {
    /// Monotonic message sequence within the session.
    pub seq: i64,
    /// Author of the message.
    pub role: Role,
    /// Message text.
    pub body: String,
    /// Optional Unix timestamp supplied by the backend.
    #[serde(default)]
    pub ts: Option<i64>,
    /// Cycle that produced or consumed the message.
    #[serde(default)]
    pub cycle_id: Option<String>,
}

/// Result of `POST /medulla/v1/sessions/:id/abort`.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AbortResult {
    /// Identifier of the targeted session.
    pub session_id: String,
    /// Whether an active cycle was aborted.
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
    /// Optional persisted sequence; streaming-only events may omit it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub seq: Option<u64>,
    /// Event timestamp supplied by the backend.
    pub at: u64,
    /// Session that owns the event.
    #[serde(rename = "sessionId")]
    pub session_id: String,
    /// Cycle associated with the event, when applicable.
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
    User {
        /// Recorded user text.
        body: String,
    },
    /// The assistant produced a final message.
    Assistant {
        /// Final assistant text.
        body: String,
    },
    /// A cognitive cycle started.
    CycleStart {
        /// Backend cycle identifier.
        cycle_id: Option<String>,
    },
    /// A cognitive cycle ended.
    CycleEnd {
        /// Backend cycle identifier.
        cycle_id: Option<String>,
        /// Number of orchestration passes consumed.
        pass_count: Option<u64>,
        /// Wall-clock duration reported by the backend.
        duration_ms: Option<u64>,
        /// Whether the cycle terminated with an error.
        error: Option<bool>,
    },
    /// An error occurred during a cycle.
    Error {
        /// Component that reported the error.
        source: String,
        /// Human-readable error detail.
        message: String,
    },
    /// Streaming assistant token delta (unpersisted, no seq).
    AssistantDelta {
        /// Newly emitted assistant text.
        delta: String,
    },
    /// Streaming reasoning token delta (unpersisted, no seq).
    ReasoningDelta {
        /// Newly emitted reasoning text.
        delta: String,
    },
    /// Streaming tool-call delta (unpersisted); raw payload preserved.
    ToolCallDelta {
        /// Raw tool-call delta payload.
        value: Value,
    },
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
    /// Name exposed to the orchestrator.
    pub name: String,
    /// Human-readable purpose used for tool selection.
    pub description: String,
    /// JSON-Schema object describing the tool parameters.
    pub parameters: Value,
}

/// A tool call requested by the orchestrator.
#[derive(Debug, Clone, Deserialize)]
pub struct ToolCall {
    /// Identifier used to correlate the eventual result.
    pub id: String,
    /// Requested tool name.
    pub name: String,
    /// JSON arguments supplied by the orchestrator.
    #[serde(default)]
    pub args: Value,
}

/// A tool result fed back via `run/continue`.
#[derive(Debug, Clone, Serialize)]
pub struct ToolResult {
    /// Identifier of the tool call being answered.
    pub id: String,
    /// Whether the tool completed successfully.
    pub ok: bool,
    /// Successful JSON result, when present.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<Value>,
    /// Failure detail, when present.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// Config knobs for a run (`options.config`).
#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RunConfig {
    /// Maximum orchestration passes.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_passes: Option<u32>,
    /// Maximum execution steps.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_steps: Option<u32>,
    /// Maximum delegation depth.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_depth: Option<u32>,
    /// Context window used for prompt budgeting.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub context_window_tokens: Option<u32>,
    /// Verification policy or mode understood by the backend.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub verification: Option<String>,
}

/// Resource limits for a run (`options.limits`).
#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RunLimits {
    /// Maximum tasks that may execute concurrently.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_concurrency: Option<u32>,
    /// Aggregate token ceiling.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_tokens: Option<u64>,
    /// Absolute or relative deadline in milliseconds, per backend contract.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub deadline_ms: Option<u64>,
    /// Maximum child tasks created by one delegate.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_tasks_per_delegate: Option<u32>,
    /// Maximum nested delegation depth.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_depth: Option<u32>,
}

/// One workspace's authored `MEDULLA.md`, sent verbatim on a run request. The
/// medulla SDK owns the format, so the text is forwarded unparsed and the
/// backend distils it into the orchestrator's context.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkspaceProfileInput {
    /// The workspace/repo path this profile describes.
    pub workspace: String,
    /// Verbatim `MEDULLA.md` contents.
    pub medulla_md: String,
}

/// The `options` object of a run request.
#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RunOrchestrationOptions {
    /// Prompt sections replaced for this run, keyed by backend section name.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub prompt_overrides: Option<std::collections::BTreeMap<String, String>>,
    /// Optional orchestration behavior overrides.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub config: Option<RunConfig>,
    /// Optional resource ceilings.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub limits: Option<RunLimits>,
    /// Authored workspace profiles for the directories this cycle works over.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub workspace_profiles: Option<Vec<WorkspaceProfileInput>>,
}

/// Optional inputs to [`crate::client::MedullaClient::run`].
#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RunOptions {
    /// Existing session to continue, or `None` to create one.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
    /// Client-side tools available to the orchestrator.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tools: Option<Vec<ToolDef>>,
    /// Nested orchestration configuration.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub options: Option<RunOrchestrationOptions>,
}

/// Final reply from a tool-less run.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RunReply {
    /// Final assistant response.
    pub reply: String,
    /// Number of orchestration passes consumed.
    #[serde(default)]
    pub pass_count: Option<u32>,
    /// Backend-produced compact history records.
    #[serde(default)]
    pub compressed_history: Vec<Value>,
    /// Escalations recorded during the run.
    #[serde(default)]
    pub escalations: Vec<Value>,
    /// Session that owns the run.
    #[serde(default)]
    pub session_id: Option<String>,
    /// Final cycle identifier.
    #[serde(default)]
    pub cycle_id: Option<String>,
}

/// A single step of the client tool-loop.
#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "stop", rename_all = "snake_case")]
pub enum LoopEvent {
    /// The orchestrator wants the client to run tools and continue.
    ToolUse {
        /// Cycle awaiting tool results.
        #[serde(rename = "cycleId")]
        cycle_id: String,
        /// Session that owns the cycle.
        #[serde(rename = "sessionId")]
        session_id: String,
        /// Calls the client must execute before continuing.
        #[serde(rename = "toolCalls", default)]
        tool_calls: Vec<ToolCall>,
    },
    /// The run finished with a final reply.
    End {
        /// Completed cycle identifier.
        #[serde(rename = "cycleId")]
        cycle_id: String,
        /// Session that owns the completed run.
        #[serde(rename = "sessionId")]
        session_id: String,
        /// Final assistant response.
        reply: String,
        /// Number of orchestration passes consumed.
        #[serde(rename = "passCount", default)]
        pass_count: Option<u32>,
        /// Backend-produced compact history records.
        #[serde(rename = "compressedHistory", default)]
        compressed_history: Vec<Value>,
        /// Escalations recorded during the run.
        #[serde(default)]
        escalations: Vec<Value>,
    },
    /// Long-poll returned without progress; poll `run/continue` again.
    Pending {
        /// Cycle still being polled.
        #[serde(rename = "cycleId")]
        cycle_id: String,
        /// Session that owns the cycle.
        #[serde(rename = "sessionId")]
        session_id: String,
    },
    /// The run errored.
    Error {
        /// Failed cycle identifier.
        #[serde(rename = "cycleId")]
        cycle_id: String,
        /// Session that owns the failed cycle.
        #[serde(rename = "sessionId")]
        session_id: String,
        /// Structured backend error payload.
        error: Value,
    },
}

/// Outcome of [`crate::client::MedullaClient::run`]: either a final reply (tool-less)
/// or a tool-loop event (when tools were supplied).
#[derive(Debug, Clone)]
pub enum RunResult {
    /// Completed tool-less run.
    Reply(RunReply),
    /// Next state in a client-managed tool loop.
    Loop(LoopEvent),
}

// ---------------------------------------------------------------------------
// History rewards (/agent-integrations/history-rewards)
// ---------------------------------------------------------------------------

/// Running claim metrics returned after uploading one transcript
/// (`POST /agent-integrations/history-rewards/uploads`).
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HistoryUploadResult {
    /// Transcripts accumulated on the claim so far.
    pub session_count: u64,
    /// Running token total across the claim.
    pub cumulative_tokens: u64,
    /// Running distinct-active-day count across the claim.
    pub active_days: u64,
    /// Agents represented on the claim so far.
    #[serde(default)]
    pub agents: Vec<String>,
}

/// Per-metric USD contributions, so the reveal can show how a total was earned.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HistoryRewardBreakdown {
    /// USD attributed to transcript token volume.
    pub tokens_usd: f64,
    /// USD attributed to distinct active days.
    pub active_days_usd: f64,
    /// USD attributed to uploaded session count.
    pub sessions_usd: f64,
    /// USD attributed to multi-agent usage.
    pub multi_agent_usd: f64,
}

/// The caller's history-reward status
/// (`GET /agent-integrations/history-rewards/status`).
///
/// This is the authority for "has this user already earned the reward?" — the
/// local config flag only decides whether to re-render the welcome screen.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HistoryRewardStatus {
    /// True once the reward has been granted.
    pub claimed: bool,
    /// True when at least one transcript was uploaded but not yet claimed.
    #[serde(default)]
    pub has_uploads: bool,
    /// USD granted, populated once scored.
    #[serde(default)]
    pub awarded_usd: f64,
    /// Human-facing power-level label.
    #[serde(default)]
    pub tier: Option<String>,
    /// Number of transcripts accumulated on the claim.
    #[serde(default)]
    pub session_count: u64,
    /// Total tokens represented by uploaded transcripts.
    #[serde(default)]
    pub cumulative_tokens: u64,
    /// Number of distinct active days represented.
    #[serde(default)]
    pub active_days: u64,
    /// Agent harnesses represented by the claim.
    #[serde(default)]
    pub agents: Vec<String>,
    /// Advertised ceiling, so the client renders "x of $25" without hardcoding.
    #[serde(default)]
    pub max_reward_usd: f64,
}

/// Result of claiming the reward
/// (`POST /agent-integrations/history-rewards/claim`).
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HistoryRewardClaim {
    /// The settled status after claiming.
    #[serde(flatten)]
    pub status: HistoryRewardStatus,
    /// How the award was composed.
    #[serde(default)]
    pub breakdown: HistoryRewardBreakdown,
    /// True when this call granted nothing new (a repeat claim).
    #[serde(default)]
    pub already_claimed: bool,
}

#[allow(unused_imports)]
use super::*;

/// Client for the Medulla backend HTTP + SSE API.
///
/// Holds the shared [`tinyhumans_sdk::TinyHumansClient`] every request is
/// issued through. `base_url`, `jwt`, and `http` are retained alongside it
/// because the SDK does not expose them back and the SSE stream — which has no
/// SDK equivalent — has to build its own authenticated URL. `default_headers`
/// keeps shared product attribution consistent across those two transports.
#[derive(Clone)]
pub struct MedullaClient {
    pub(super) base_url: String,
    pub(super) jwt: String,
    pub(super) http: reqwest::Client,
    pub(super) default_headers: reqwest::header::HeaderMap,
    pub(super) sdk: tinyhumans_sdk::TinyHumansClient,
}

/// Hand-written because [`tinyhumans_sdk::TinyHumansClient`] is not `Debug`.
/// The JWT is deliberately not printed.
impl std::fmt::Debug for MedullaClient {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MedullaClient")
            .field("base_url", &self.base_url)
            .field("authenticated", &!self.jwt.is_empty())
            .finish_non_exhaustive()
    }
}

/// Builder for [`MedullaClient`].
#[derive(Debug, Default)]
pub struct MedullaClientBuilder {
    pub(super) base_url: Option<String>,
    pub(super) jwt: Option<String>,
    pub(super) http: Option<reqwest::Client>,
}
