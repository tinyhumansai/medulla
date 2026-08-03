//! Version 2 of the harness session envelope — a typed event stream.
//!
//! Additive over [`v1`](super::v1): it reuses the `bucket`/`scope`/`harness`/
//! `source` blocks verbatim and swaps v1's single `message` object for a typed
//! [`HarnessEvent`] carrying a `kind` discriminator plus a per-kind `payload`.
//! The two versions coexist; consumers discriminate on `envelope_version`.
//!
//! The wire format is **snake_case** for the same reason as v1, and for the same
//! reason must stay that way.

use serde::{Deserialize, Serialize};

use super::v1::{HarnessBucket, HarnessInfo, HarnessScope, HarnessSource};

/// Which side of the master chat an event belongs to: `"owner"` or `"agent"`.
pub type HarnessEventRole = String;

/// The normalized tool family: `shell`, `file_read`, `file_write`, `edit`,
/// `search`, `web`, `mcp`, `task` or `other`.
pub type HarnessToolKind = String;

/// The derived activity state: `running`, `running_tool`, `waiting_approval`,
/// `idle`, `stopped` or `errored`.
pub type HarnessSessionState = String;

/// `envelope_version` discriminator for v2 envelopes.
///
/// A wire value; see [`super::v1::SESSION_ENVELOPE_VERSION_V1`].
pub const SESSION_ENVELOPE_VERSION_V2: &str = "tinyplace.harness.session.v2";

/// `user_prompt` payload.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct UserPromptPayload {
    /// The prompt text.
    #[serde(default)]
    pub text: String,
    /// `"human"` or `"openhuman_inject"`.
    #[serde(default)]
    pub source: String,
}

/// Shared payload for the text-only kinds (`agent_message`, `agent_thinking`).
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct TextPayload {
    /// The text.
    #[serde(default)]
    pub text: String,
}

/// `tool_call` payload.
///
/// `tool_kind` is intentionally a `String` rather than an enum so an
/// unrecognised tool family does not fail the whole event decode.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct ToolCallPayload {
    /// Correlates with the matching [`ToolResultPayload::call_id`].
    #[serde(default)]
    pub call_id: String,
    /// Raw harness name, e.g. `"Bash"`.
    #[serde(default)]
    pub tool_name: String,
    /// Normalized family, kept free-form for forward compatibility.
    #[serde(default)]
    pub tool_kind: String,
    /// One-line human summary, e.g. `"npm test"`.
    #[serde(default)]
    pub display: String,
    /// Bounded and redacted by the tailer before publish.
    #[serde(default)]
    pub input: serde_json::Value,
}

/// `tool_result` payload.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct ToolResultPayload {
    /// Correlates with the originating [`ToolCallPayload::call_id`].
    #[serde(default)]
    pub call_id: String,
    /// Whether the tool succeeded.
    #[serde(default)]
    pub ok: bool,
    /// Process exit code, when the tool had one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub exit_code: Option<i64>,
    /// Whether the harness flagged the result as an error.
    #[serde(default)]
    pub is_error: bool,
    /// Truncated output (~4KB cap); the suffix marks elision.
    #[serde(default)]
    pub output: String,
    /// Byte length of the untruncated output.
    #[serde(default)]
    pub output_bytes: i64,
}

/// `approval_request` payload.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct ApprovalRequestPayload {
    /// The call awaiting approval, when the harness supplied one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub call_id: Option<String>,
    /// Raw harness tool name.
    #[serde(default)]
    pub tool_name: String,
    /// One-line human summary of what is being approved.
    #[serde(default)]
    pub display: String,
    /// Why approval is required, when the harness said.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

/// `status` payload — the harness run-state.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct StatusPayload {
    /// `running`, `running_tool`, `waiting_approval`, `idle`, `stopped` or
    /// `errored`.
    #[serde(default)]
    pub state: String,
    /// Human-readable current activity.
    #[serde(default)]
    pub detail: String,
    /// The tool call the state refers to, when there is one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub active_call_id: Option<String>,
}

/// `lifecycle` payload.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct LifecyclePayload {
    /// `session_start`, `session_end`, `turn_start`, `turn_end` or `compact`.
    #[serde(default)]
    pub phase: String,
}

/// `error` payload.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct ErrorPayload {
    /// The error text.
    #[serde(default)]
    pub message: String,
    /// Whether the session cannot continue.
    #[serde(default)]
    pub fatal: bool,
}

/// `unknown` payload (`{ "raw": any }`).
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct UnknownPayload {
    /// The raw payload, bounded by the producer.
    #[serde(default)]
    pub raw: serde_json::Value,
}

/// The typed payload of a v2 `event`, keyed by `event.kind`.
///
/// Adjacently tagged on the wire's `kind` (discriminator) and `payload`
/// (content) fields, decoded via [`HarnessEvent::decoded`]. `snake_case` matches
/// the wire kind strings.
///
/// Not `Eq`: [`ToolCallPayload::input`] and [`UnknownPayload::raw`] are
/// `serde_json::Value`, which is not `Eq`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "payload", rename_all = "snake_case")]
pub enum HarnessEventKind {
    /// The owner prompted the harness.
    UserPrompt(UserPromptPayload),
    /// The harness said something.
    AgentMessage(TextPayload),
    /// The harness's reasoning trace.
    AgentThinking(TextPayload),
    /// The harness invoked a tool.
    ToolCall(ToolCallPayload),
    /// A tool finished.
    ToolResult(ToolResultPayload),
    /// The harness is blocked on the owner approving something.
    ApprovalRequest(ApprovalRequestPayload),
    /// A run-state transition.
    Status(StatusPayload),
    /// A session or turn boundary.
    Lifecycle(LifecyclePayload),
    /// The harness reported an error.
    Error(ErrorPayload),
    /// The `unknown` wire kind, or any forward-incompatible kind that could not
    /// be decoded — folded here rather than hard-failing the parse.
    Unknown(UnknownPayload),
}

/// A v2 `event`: common envelope fields plus the `kind`/`payload` pair.
///
/// `kind` and `payload` are kept raw here (a discriminator string plus arbitrary
/// JSON) and decoded on demand by [`HarnessEvent::decoded`], so an unknown or
/// garbled event never fails the whole envelope parse.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct HarnessEvent {
    /// Content-derived id, so a resend is idempotent.
    #[serde(default)]
    pub id: String,
    /// Monotonic per session — the ordering key.
    #[serde(default)]
    pub seq: i64,
    /// ISO-8601 timestamp.
    #[serde(default)]
    pub ts: String,
    /// The turn this event belongs to, when the harness tracks turns.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub turn_id: Option<String>,
    /// The model in use, when known.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    /// `owner` when `kind == user_prompt`, else `agent`.
    #[serde(default)]
    pub role: HarnessEventRole,
    /// The wire kind discriminator.
    #[serde(default)]
    pub kind: String,
    /// The kind-specific payload, decoded by [`HarnessEvent::decoded`].
    #[serde(default)]
    pub payload: serde_json::Value,
}

impl HarnessEvent {
    /// Decode `(kind, payload)` into a typed [`HarnessEventKind`].
    ///
    /// Forward-safe: an unrecognised `kind`, or a payload that fails its kind's
    /// schema, folds to [`HarnessEventKind::Unknown`] carrying the raw payload,
    /// so a future or garbled event never hard-fails the parse.
    pub fn decoded(&self) -> HarnessEventKind {
        let tagged = serde_json::json!({ "kind": self.kind, "payload": self.payload });
        serde_json::from_value(tagged).unwrap_or_else(|_| {
            HarnessEventKind::Unknown(UnknownPayload {
                raw: self.payload.clone(),
            })
        })
    }
}

/// The v2 envelope: the v1 `bucket`/`scope`/`harness`/`source` blocks with v1's
/// `message` replaced by a typed [`HarnessEvent`].
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SessionEnvelopeV2 {
    /// Always [`SESSION_ENVELOPE_VERSION_V2`] for a valid envelope.
    #[serde(default)]
    pub envelope_version: String,
    /// Numeric schema version (`2`).
    #[serde(default)]
    pub version: u32,
    /// Aggregation bucket.
    #[serde(default)]
    pub bucket: HarnessBucket,
    /// Session anchoring.
    #[serde(default)]
    pub scope: HarnessScope,
    /// The wrapped process.
    #[serde(default)]
    pub harness: HarnessInfo,
    /// The event this envelope carries.
    #[serde(default)]
    pub event: HarnessEvent,
    /// Where the event came from on disk.
    #[serde(default)]
    pub source: HarnessSource,
}

impl SessionEnvelopeV2 {
    /// Well-formed v2 envelope: the version tag matches and a harness session id
    /// is present.
    pub fn is_valid_v2(&self) -> bool {
        self.envelope_version == SESSION_ENVELOPE_VERSION_V2
            && !self.scope.harness_session_id.is_empty()
    }

    /// Parse a message body as a v2 session envelope.
    ///
    /// Returns `None` for any non-v2 payload (a v1 envelope or a plain message)
    /// so callers can fall through to v1 and then to their default surface.
    /// Discriminates purely on `envelope_version`, so a v1 body never matches
    /// here and vice versa.
    pub fn parse(body: &str) -> Option<Self> {
        let envelope: Self = serde_json::from_str(body).ok()?;
        envelope.is_valid_v2().then_some(envelope)
    }

    /// The per-pair routing key — identical semantics to
    /// [`super::v1::SessionEnvelopeV1::session_key`].
    pub fn session_key(&self) -> String {
        if !self.scope.wrapper_session_id.is_empty() {
            return self.scope.wrapper_session_id.clone();
        }
        self.scope.harness_session_id.clone()
    }
}
