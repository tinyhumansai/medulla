//! Tests for the control module.

use crate::protocol::{
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
