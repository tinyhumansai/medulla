//! Owner-to-machine control frames for the harness session bus.
//!
//! One machine wallet is shared by every wrapped harness session on it. Plain
//! DMs are injected into the machine's primary session; a control frame lets the
//! owner address a specific session by id — either the harness's own session id
//! or the wrapper session id (a frame may name either). Absent id targets the
//! primary session.

use serde::{Deserialize, Serialize};

/// Wire version tag stamped on every control frame body.
pub const HARNESS_CONTROL_VERSION: &str = "tinyplace.harness.control.v1";

impl HarnessControlFrame {
    /// Serialize this control frame for an encrypted DM body.
    pub fn encode(&self) -> String {
        serde_json::to_string(self).expect("HarnessControlFrame always serializes")
    }
}

/// Build and serialize an `input` control frame.
pub fn encode_harness_control_frame(text: &str, session_id: Option<&str>) -> String {
    HarnessControlFrame {
        control_version: HARNESS_CONTROL_VERSION.to_string(),
        kind: "input".to_string(),
        session_id: session_id.filter(|s| !s.is_empty()).map(str::to_string),
        text: text.to_string(),
    }
    .encode()
}

/// Decode a DM body into a [`HarnessControlFrame`], or `None` when the body is
/// not one of ours (plain text, a session envelope, another protocol, or a
/// malformed frame). Never panics — inbound bodies are untrusted.
pub fn parse_harness_control_frame(body: &str) -> Option<HarnessControlFrame> {
    if !body.trim_start().starts_with('{') {
        return None;
    }
    let value: serde_json::Value = serde_json::from_str(body).ok()?;
    let obj = value.as_object()?;

    if obj.get("control_version").and_then(|v| v.as_str()) != Some(HARNESS_CONTROL_VERSION) {
        return None;
    }
    if obj.get("kind").and_then(|v| v.as_str()) != Some("input") {
        return None;
    }
    let text = obj.get("text").and_then(|v| v.as_str())?.to_string();
    let session_id = obj
        .get("session_id")
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
        .map(str::to_string);

    Some(HarnessControlFrame {
        control_version: HARNESS_CONTROL_VERSION.to_string(),
        kind: "input".to_string(),
        session_id,
        text,
    })
}

#[cfg(test)]
mod tests;

mod types;
pub use types::HarnessControlFrame;
