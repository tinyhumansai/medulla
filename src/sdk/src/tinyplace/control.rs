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

/// A decoded control frame. `input` types `text` into the addressed session's
/// agent as a prompt.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HarnessControlFrame {
    pub control_version: String,
    pub kind: String,
    /// Target session (wrapper or harness session id). Absent targets the primary.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub session_id: Option<String>,
    pub text: String,
}

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
mod tests {
    use crate::tinyplace::{
        encode_harness_control_frame, parse_harness_control_frame, HARNESS_CONTROL_VERSION,
    };

    #[test]
    fn encodes_a_control_frame_targeting_the_primary() {
        let body = encode_harness_control_frame("hello", None);
        let value: serde_json::Value = serde_json::from_str(&body).unwrap();
        assert_eq!(value["control_version"], HARNESS_CONTROL_VERSION);
        assert_eq!(value["kind"], "input");
        assert_eq!(value["text"], "hello");
        assert!(value.get("session_id").is_none());
    }

    #[test]
    fn encodes_a_control_frame_targeting_a_session() {
        let body = encode_harness_control_frame("run tests", Some("wsid-1"));
        let frame = parse_harness_control_frame(&body).unwrap();
        assert_eq!(frame.session_id.as_deref(), Some("wsid-1"));
        assert_eq!(frame.text, "run tests");
    }

    #[test]
    fn empty_session_id_is_treated_as_absent() {
        let body = encode_harness_control_frame("x", Some(""));
        let frame = parse_harness_control_frame(&body).unwrap();
        assert_eq!(frame.session_id, None);
    }

    #[test]
    fn parse_rejects_non_frames() {
        // Not JSON / not an object.
        assert!(parse_harness_control_frame("plain text dm").is_none());
        assert!(parse_harness_control_frame("  not { json").is_none());
        // Wrong version tag.
        assert!(parse_harness_control_frame(
            r#"{"control_version":"other","kind":"input","text":"x"}"#
        )
        .is_none());
        // Wrong kind.
        assert!(parse_harness_control_frame(&format!(
            r#"{{"control_version":"{HARNESS_CONTROL_VERSION}","kind":"status","text":"x"}}"#
        ))
        .is_none());
        // Missing text.
        assert!(parse_harness_control_frame(&format!(
            r#"{{"control_version":"{HARNESS_CONTROL_VERSION}","kind":"input"}}"#
        ))
        .is_none());
    }

    #[test]
    fn parse_tolerates_leading_whitespace() {
        let frame = parse_harness_control_frame(&format!(
        "\n  {{\"control_version\":\"{HARNESS_CONTROL_VERSION}\",\"kind\":\"input\",\"text\":\"hi\"}}"
    ))
    .unwrap();
        assert_eq!(frame.text, "hi");
    }
}
