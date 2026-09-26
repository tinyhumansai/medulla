//! The version-tagged envelope `medulla.remote.v1` shares the channel with.
//!
//! Deliberately the same shape as [`screen::codec`](super::super::screen::codec),
//! including the cheap pre-checks: several protocols arrive on one channel, and
//! each parser must answer only for its own and fall through silently for
//! everything else rather than mangling someone else's body into one of ours.

use super::types::{RemoteEnvelope, RemoteMessage, REMOTE_PROTO};

/// Serialize `message` for a link body, stamping the version tag.
pub fn encode_remote_message(message: &RemoteMessage) -> String {
    let envelope = RemoteEnvelope {
        remote_version: REMOTE_PROTO.to_string(),
        message: message.clone(),
    };
    serde_json::to_string(&envelope).expect("RemoteEnvelope always serializes")
}

/// Decode a link body into a [`RemoteMessage`], or `None` when it is not one of
/// ours.
///
/// Never panics: inbound bodies are untrusted, so a malformed or foreign body is
/// a `None` for the next parser to try, not an error to propagate.
pub fn parse_remote_message(body: &str) -> Option<RemoteMessage> {
    if !body.trim_start().starts_with('{') {
        return None;
    }
    let value: serde_json::Value = serde_json::from_str(body).ok()?;
    if value.get("remote_version").and_then(|v| v.as_str()) != Some(REMOTE_PROTO) {
        return None;
    }
    serde_json::from_value::<RemoteEnvelope>(value)
        .ok()
        .map(|envelope| envelope.message)
}
