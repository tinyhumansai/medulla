//! Restricted workflow-tool mode behavior on the task-frame wire.

use crate::tinyplace::{decode_task_frame, encode_task_frame, EncodeFrameInput, TaskFrameKind};

#[test]
fn the_tool_mode_survives_a_frame_round_trip() {
    let body = encode_task_frame(EncodeFrameInput {
        kind: TaskFrameKind::Task,
        task_id: "t1".into(),
        text: "review it".into(),
        ts: "2026-01-01T00:00:00Z".into(),
        correlation_id: None,
        harness: None,
        provider: None,
        custom_harness: None,
        model: None,
        tool_mode: Some("propose:sweep".into()),
        workflow: None,
        conversation: None,
        fleet_depth: 0,
    });

    let decoded = decode_task_frame(&body).expect("the frame decodes");
    assert_eq!(decoded.tool_mode.as_deref(), Some("propose:sweep"));
}

#[test]
fn a_frame_from_a_peer_that_predates_tool_modes_still_decodes() {
    assert!(decode_task_frame(&frame(serde_json::json!({})))
        .expect("the frame decodes")
        .tool_mode
        .is_none());
}

#[test]
fn a_blank_tool_mode_is_absent_rather_than_a_mode_named_nothing() {
    assert!(
        decode_task_frame(&frame(serde_json::json!({ "tool_mode": "   " })))
            .expect("the frame decodes")
            .tool_mode
            .is_none()
    );
}

#[test]
fn malformed_tool_modes_reject_the_frame() {
    for malformed in [
        serde_json::json!("unrestricted"),
        serde_json::json!("propose:"),
        serde_json::json!(true),
        serde_json::json!(7),
    ] {
        assert!(decode_task_frame(&frame(serde_json::json!({
            "tool_mode": malformed
        })))
        .is_none());
    }
}

fn frame(extra: serde_json::Value) -> String {
    let mut frame = serde_json::json!({
        "proto": crate::tinyplace::TINYPLACE_PROTO,
        "kind": "task",
        "taskId": "t1",
        "text": "do it",
        "ts": "2026-01-01T00:00:00Z"
    });
    frame
        .as_object_mut()
        .expect("frame object")
        .extend(extra.as_object().expect("extra object").clone());
    frame.to_string()
}
