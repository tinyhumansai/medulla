//! Tests for the control module.

use super::*;
use crate::tinyplace::parse_harness_control_frame;

fn frame(session_id: Option<&str>) -> HarnessControlFrame {
    HarnessControlFrame {
        control_version: crate::tinyplace::HARNESS_CONTROL_VERSION.to_string(),
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
        "control_version": crate::tinyplace::HARNESS_CONTROL_VERSION,
        "kind": "input",
        "text": "hello",
    })
    .to_string();
    let parsed = parse_harness_control_frame(&body).unwrap();
    assert!(frame_targets_session(&parsed, "w", "h"));
}
