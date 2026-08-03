//! Version 1 of the harness session envelope — a single semantic `message`.
//!
//! The wire format is **snake_case**, unlike the camelCase used elsewhere: the
//! envelope is produced by the CLI wrapper as a literal snake_case object, so
//! these structs deliberately omit `#[serde(rename_all = "camelCase")]`. Do not
//! "normalize" them — it breaks decoding of envelopes already in flight.

use serde::{Deserialize, Serialize};

/// `envelope_version` discriminator for v1 envelopes.
///
/// A wire value. Deployed wrappers emit this exact string; changing it makes
/// every envelope they send unparseable here.
pub const SESSION_ENVELOPE_VERSION_V1: &str = "tinyplace.harness.session.v1";

/// The harness that produced an envelope: `"codex"` or `"claude"`.
///
/// Kept as a bare `String` on the envelope; [`crate::protocol::HarnessProvider`]
/// is the typed provider of the task-frame protocol and is a different thing.
pub type HarnessProvider = String;

/// Who authored a v1 message: `"user"` or `"agent"`.
pub type HarnessMessageRole = String;

/// The aggregation bucket unit: `"minute"`, `"hour"` or `"day"`.
pub type HarnessBucketUnit = String;

/// What a session is anchored to: `"folder"` or `"session"`.
pub type HarnessEnvelopeScope = String;

/// Rate/aggregation bucket the envelope belongs to.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct HarnessBucket {
    /// The bucket granularity.
    #[serde(default)]
    pub unit: HarnessBucketUnit,
    /// Bucket start, ISO-8601.
    #[serde(default)]
    pub start: String,
    /// Bucket end, ISO-8601.
    #[serde(default)]
    pub end: String,
}

/// Where the wrapped session is anchored (folder- or session-scoped).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct HarnessScope {
    /// `folder` or `session`.
    #[serde(rename = "type", default)]
    pub scope_type: HarnessEnvelopeScope,
    /// Scope key — the folder or session identifier.
    #[serde(default)]
    pub key: String,
    /// Working directory of the wrapped process.
    #[serde(default)]
    pub cwd: String,
    /// The shared per-pair conversation id; both peers reuse it on reply.
    #[serde(default)]
    pub wrapper_session_id: String,
    /// The harness's own session id.
    #[serde(default)]
    pub harness_session_id: String,
}

/// The wrapped harness process (provider + invocation).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct HarnessInfo {
    /// `codex` or `claude`.
    #[serde(default)]
    pub provider: HarnessProvider,
    /// The binary that was executed.
    #[serde(default)]
    pub command: String,
    /// Full argument vector, command included.
    #[serde(default)]
    pub argv: Vec<String>,
}

/// A single semantic message from the wrapped session.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct HarnessMessage {
    /// Stable message id, used for dedup on resend.
    #[serde(default)]
    pub id: String,
    /// Line number in the source transcript.
    #[serde(default)]
    pub line: i64,
    /// `user` or `agent`.
    #[serde(default)]
    pub role: HarnessMessageRole,
    /// The message body.
    #[serde(default)]
    pub text: String,
    /// ISO-8601 timestamp.
    #[serde(default)]
    pub timestamp: String,
    /// Optional lifecycle phase this message belongs to.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub phase: Option<String>,
}

/// Provenance of the message on the wrapper's disk.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct HarnessSource {
    /// Transcript path the record was read from.
    #[serde(default)]
    pub path: String,
    /// The transcript's own record type, e.g. `assistant`.
    #[serde(default)]
    pub record_type: String,
    /// The role the source record claimed, when it differs from `message.role`.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub source_role: Option<String>,
}

/// The versioned v1 wire schema a wrapped session forwards as a message body.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SessionEnvelopeV1 {
    /// Always [`SESSION_ENVELOPE_VERSION_V1`] for a valid envelope.
    #[serde(default)]
    pub envelope_version: String,
    /// Numeric schema version (`1`).
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
    /// The message this envelope carries.
    #[serde(default)]
    pub message: HarnessMessage,
    /// Where the message came from on disk.
    #[serde(default)]
    pub source: HarnessSource,
}

/// The current default envelope version for callers that do not discriminate.
pub type SessionEnvelope = SessionEnvelopeV1;

impl SessionEnvelopeV1 {
    /// Well-formed v1 envelope: the version tag matches and a harness session id
    /// is present.
    pub fn is_valid_v1(&self) -> bool {
        self.envelope_version == SESSION_ENVELOPE_VERSION_V1
            && !self.scope.harness_session_id.is_empty()
    }

    /// Parse a message body as a v1 session envelope.
    ///
    /// Returns `None` for any non-envelope payload (a plain message) or a
    /// wrong/absent version, so callers can route those to their default
    /// surface.
    pub fn parse(body: &str) -> Option<Self> {
        let envelope: Self = serde_json::from_str(body).ok()?;
        envelope.is_valid_v1().then_some(envelope)
    }

    /// The single per-pair session id to bucket an inbound message under.
    ///
    /// Every message — whoever sent it — carries exactly one id: the shared
    /// conversation id in `scope.wrapper_session_id`. Both peers put the same id
    /// there for a given thread, so it is the sole routing key. Falls back to
    /// `harness_session_id` only for a legacy envelope that predates it.
    pub fn session_key(&self) -> String {
        if !self.scope.wrapper_session_id.is_empty() {
            return self.scope.wrapper_session_id.clone();
        }
        self.scope.harness_session_id.clone()
    }

    /// Build an outgoing v1 envelope carrying `body` under `session_id`, so a
    /// compliant peer harness threads its reply under the same session.
    pub fn outgoing(session_id: &str, body: &str, message_id: &str, timestamp: &str) -> Self {
        SessionEnvelopeV1 {
            envelope_version: SESSION_ENVELOPE_VERSION_V1.to_string(),
            version: 1,
            scope: HarnessScope {
                scope_type: "session".to_string(),
                wrapper_session_id: session_id.to_string(),
                harness_session_id: session_id.to_string(),
                ..Default::default()
            },
            message: HarnessMessage {
                id: message_id.to_string(),
                role: "owner".to_string(),
                text: body.to_string(),
                timestamp: timestamp.to_string(),
                ..Default::default()
            },
            ..Default::default()
        }
    }
}
