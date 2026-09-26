//! Serializing screen messages into encrypted DM bodies, and recognising them
//! on the way back.
//!
//! This channel carries four protocols — task frames, harness control frames,
//! session envelopes, and these — so decoding is first a question of *whose*
//! body this is. [`parse_screen_message`] answers only for its own and never
//! panics: inbound bodies are untrusted, and a body that is not ours must fall
//! through to the next parser rather than being mangled into one.

use super::types::{ScreenEnvelope, ScreenMessage, SCREEN_PROTO};

/// Serialize `message` for an encrypted DM body, stamping the version tag.
pub fn encode_screen_message(message: &ScreenMessage) -> String {
    let envelope = ScreenEnvelope {
        screen_version: SCREEN_PROTO.to_string(),
        message: message.clone(),
    };
    serde_json::to_string(&envelope).expect("ScreenEnvelope always serializes")
}

/// Decode a DM body into a [`ScreenMessage`], or `None` when the body is not one
/// of ours — plain text, another protocol, or a malformed frame.
///
/// The cheap checks come first: anything that is not a JSON object carrying our
/// exact version tag is rejected before the full deserialize is attempted, so
/// the common case (someone else's frame) costs almost nothing.
pub fn parse_screen_message(body: &str) -> Option<ScreenMessage> {
    if !body.trim_start().starts_with('{') {
        return None;
    }
    let value: serde_json::Value = serde_json::from_str(body).ok()?;
    if value.get("screen_version").and_then(|v| v.as_str()) != Some(SCREEN_PROTO) {
        return None;
    }
    serde_json::from_value::<ScreenEnvelope>(value)
        .ok()
        .map(|envelope| envelope.message)
}
