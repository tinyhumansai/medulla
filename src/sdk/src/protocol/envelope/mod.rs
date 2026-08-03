//! The harness session-envelope wire model.
//!
//! This is what a wrapped Codex / Claude / OpenCode session forwards as a
//! message body: a versioned JSON object describing one thing the harness did.
//! `consumer/`, `status/`, `harness_work/fold.rs`, `sessions/input/` and the
//! daemon mappers all fold over these types, and the daemon and the wrapper are
//! the two ends of the format.
//!
//! Two versions coexist and are discriminated on `envelope_version`:
//!
//! - [`v1`] — one semantic `message` per envelope.
//! - [`v2`] — a typed [`HarnessEvent`] with a `kind` discriminator and a
//!   per-kind payload.
//!
//! [`AnySessionEnvelope`] accepts either.
//!
//! # The serde representation is a wire format
//!
//! These structs are snake_case, unlike the camelCase used elsewhere in the
//! codebase, because the wrapper emits a literal snake_case object. The two
//! `envelope_version` strings are likewise literal wire values. Renaming a
//! field, changing a version string, or "normalizing" the casing silently breaks
//! decoding of envelopes produced by every already-deployed wrapper — the
//! failure looks like a session that simply never reports anything.

mod v1;
mod v2;

#[cfg(test)]
mod tests;

use serde::Serialize;

pub use v1::{
    HarnessBucket, HarnessBucketUnit, HarnessEnvelopeScope, HarnessInfo, HarnessMessage,
    HarnessMessageRole, HarnessScope, HarnessSource, SessionEnvelope, SessionEnvelopeV1,
    SESSION_ENVELOPE_VERSION_V1,
};
pub use v2::{
    ApprovalRequestPayload, ErrorPayload, HarnessEvent, HarnessEventKind, HarnessEventRole,
    HarnessSessionState, HarnessToolKind, LifecyclePayload, SessionEnvelopeV2, StatusPayload,
    TextPayload, ToolCallPayload, ToolResultPayload, UnknownPayload, UserPromptPayload,
    SESSION_ENVELOPE_VERSION_V2,
};

/// Either version of a harness session envelope, for consumers that accept both.
///
/// Serialization is `untagged` (the inner envelope's own fields, verbatim).
/// Deserialization goes through [`AnySessionEnvelope::parse`], which
/// discriminates on `envelope_version` — a plain serde untagged decode would be
/// ambiguous, since both structs accept `{}` (every field is
/// `#[serde(default)]`).
#[derive(Debug, Clone, Serialize)]
#[serde(untagged)]
pub enum AnySessionEnvelope {
    /// A typed-event envelope.
    V2(SessionEnvelopeV2),
    /// A single-message envelope.
    V1(SessionEnvelopeV1),
}

impl AnySessionEnvelope {
    /// Parse a message body as either envelope version.
    ///
    /// Tries v2 first (the richer schema), then v1; returns `None` for any
    /// non-envelope payload so callers route it to their default surface.
    pub fn parse(body: &str) -> Option<Self> {
        if let Some(v2) = SessionEnvelopeV2::parse(body) {
            return Some(AnySessionEnvelope::V2(v2));
        }
        SessionEnvelopeV1::parse(body).map(AnySessionEnvelope::V1)
    }
}
