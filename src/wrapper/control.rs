//! Owner→wrapper control-frame targeting.
//!
//! The wrapper polls its mailbox for [`HarnessControlFrame`]s and injects their
//! `text` into the child. A frame may address a specific session by id (either
//! the wrapper session id or the harness's own session id); an absent id targets
//! the single session this terminal runs. Since this is a single-terminal wrapper
//! (the machine-bus multi-terminal router is a scope cut), matching is a direct
//! id comparison rather than a spool lookup.

use crate::tinyplace_support::HarnessControlFrame;

/// Whether `frame` addresses this wrapper's session. A frame with no `session_id`
/// always matches (there is only one session here); a frame naming an id matches
/// only its wrapper or harness id.
pub fn frame_targets_session(
    frame: &HarnessControlFrame,
    wrapper_session_id: &str,
    harness_session_id: &str,
) -> bool {
    match frame.session_id.as_deref() {
        None => true,
        Some(id) => id == wrapper_session_id || id == harness_session_id,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tinyplace_support::parse_harness_control_frame;

    fn frame(session_id: Option<&str>) -> HarnessControlFrame {
        HarnessControlFrame {
            control_version: crate::tinyplace_support::HARNESS_CONTROL_VERSION.to_string(),
            kind: "input".to_string(),
            session_id: session_id.map(str::to_string),
            text: "run tests".to_string(),
        }
    }

    #[test]
    fn absent_id_targets_the_single_session() {
        assert!(frame_targets_session(&frame(None), "wsid", "hsid"));
    }

    #[test]
    fn matches_wrapper_or_harness_id_only() {
        assert!(frame_targets_session(&frame(Some("wsid")), "wsid", "hsid"));
        assert!(frame_targets_session(&frame(Some("hsid")), "wsid", "hsid"));
        assert!(!frame_targets_session(
            &frame(Some("other")),
            "wsid",
            "hsid"
        ));
    }

    #[test]
    fn parses_and_targets_a_wire_frame() {
        let body = serde_json::json!({
            "control_version": crate::tinyplace_support::HARNESS_CONTROL_VERSION,
            "kind": "input",
            "text": "hello",
        })
        .to_string();
        let parsed = parse_harness_control_frame(&body).unwrap();
        assert!(frame_targets_session(&parsed, "w", "h"));
    }
}
