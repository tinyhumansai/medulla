//! JSON types for the Medulla client surface.
//!
//! Most of these now live in `tinyhumans-sdk` and are re-exported below, so the
//! shared SDK owns one definition of the backend contract. What stays here is
//! what the SDK does not model:
//!
//! - the history-reward payloads, where this crate needs the full settled
//!   status and per-metric breakdown the reveal screen renders, while the SDK
//!   exposes a narrower projection;
//! - [`RunOptions`], the argument shape of [`crate::client::MedullaClient::run`],
//!   which wraps the SDK's own `RunOptions` rather than mirroring it;
//! - the client and builder structs themselves.

use serde::{Deserialize, Serialize};

/// One workspace's authored `MEDULLA.md`, sent verbatim on a run request.
pub use tinyhumans_sdk::api::medulla::WorkspaceProfile as WorkspaceProfileInput;
pub use tinyhumans_sdk::api::medulla::{
    AbortResult, EventEnvelope, EventKind, Message, Role, SendResult, SessionArchived,
    SessionCreated, SessionDetail, SessionStatus, SessionSummary,
};
/// The `options` object of a run request.
pub use tinyhumans_sdk::api::orchestration::RunOptions as RunOrchestrationOptions;
pub use tinyhumans_sdk::api::orchestration::{
    LoopEvent, RunConfig, RunLimits, RunReply, RunResult, ToolCall, ToolDef, ToolResult,
};

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

/// Optional inputs to [`crate::client::MedullaClient::run`].
///
/// Distinct from the SDK's `RunOptions`, which models only the nested
/// `options` object; this is the whole request minus the input text.
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
/// SDK equivalent — has to build its own authenticated URL.
#[derive(Clone)]
pub struct MedullaClient {
    pub(super) base_url: String,
    pub(super) jwt: String,
    pub(super) http: reqwest::Client,
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
