//! Frame-attachment tests: the session id a reply reports back, and the
//! transport flavor a task carries across the wire.
//!
//! Moved from `codec.rs` (see AGENTS.md's 500-line ceiling); these exercise
//! `encode_task_frame_with_attachments` and `FrameAttachments` rather than the
//! core encode/decode round-trip `codec.rs` keeps.

use crate::protocol::{
    decode_task_frame, encode_task_frame, EncodeFrameInput, HarnessProvider, HarnessTransport,
    TaskFrameKind, MEDULLA_TASK_PROTO,
};
use serde_json::json;

/// A response says which session served the task. Reported, never requested:
/// this is the only path by which a session id travels back to a caller, which
/// otherwise has no way to name where its work happened.
#[test]
fn a_response_reports_the_session_that_served_the_task() {
    let body = crate::protocol::encode_task_frame_with_attachments(
        reply_input(),
        crate::protocol::FrameAttachments {
            session_id: Some("sess-42".to_string()),
            ..Default::default()
        },
    );
    let value: serde_json::Value = serde_json::from_str(&body).unwrap();
    assert_eq!(value["sessionId"], "sess-42");

    let decoded = decode_task_frame(&body).expect("a valid frame");
    assert_eq!(decoded.session_id.as_deref(), Some("sess-42"));
}

/// Blank is not a session, in either direction: an encoder that always writes
/// the key must not claim `""`, and a peer that sends one must not have it
/// recorded as a session id nothing can resume.
#[test]
fn a_blank_session_id_is_absent_rather_than_empty() {
    let body = crate::protocol::encode_task_frame_with_attachments(
        reply_input(),
        crate::protocol::FrameAttachments {
            session_id: Some("   ".to_string()),
            ..Default::default()
        },
    );
    let value: serde_json::Value = serde_json::from_str(&body).unwrap();
    assert!(value.get("sessionId").is_none());

    let sent_blank = json!({
        "proto": MEDULLA_TASK_PROTO,
        "kind": "reply",
        "taskId": "cycle-1",
        "text": "done",
        "ts": "2026-07-18T00:00:00.000Z",
        "sessionId": "  ",
    })
    .to_string();
    assert_eq!(decode_task_frame(&sent_blank).unwrap().session_id, None);
}

/// An ordinary outbound task never claims a session. Session *targeting* is a
/// separate feature with a separate trust story — one caller must not be able to
/// run inside a session opened for another — so the key stays off the request
/// side entirely.
#[test]
fn a_dispatched_task_names_no_session() {
    let body = encode_task_frame(EncodeFrameInput {
        kind: TaskFrameKind::Task,
        task_id: "cycle-1".to_string(),
        text: "do the thing".to_string(),
        ts: "2026-07-18T00:00:00.000Z".to_string(),
        correlation_id: None,
        harness: None,
        provider: None,
        transport: None,
        custom_harness: None,
        model: None,
        tool_mode: None,
        workflow: None,
        workflow_fingerprint: None,
        workflow_inputs: Default::default(),
        conversation: None,
        fleet_depth: 0,
    });
    let value: serde_json::Value = serde_json::from_str(&body).unwrap();
    assert!(value.get("sessionId").is_none());
}

/// A `reply` frame's inputs, for the session-reporting cases.
fn reply_input() -> EncodeFrameInput {
    EncodeFrameInput {
        kind: TaskFrameKind::Reply,
        task_id: "cycle-1".to_string(),
        text: "done".to_string(),
        ts: "2026-07-18T00:00:00.000Z".to_string(),
        correlation_id: None,
        harness: None,
        provider: None,
        transport: None,
        custom_harness: None,
        model: None,
        tool_mode: None,
        workflow: None,
        workflow_fingerprint: None,
        workflow_inputs: Default::default(),
        conversation: None,
        fleet_depth: 0,
    }
}

/// The flavor has to survive the wire, or a worker runs the CLI for a sender who
/// asked for the shared process and neither end can tell.
#[test]
fn carries_a_non_default_transport_across_the_wire() {
    let body = encode_task_frame(EncodeFrameInput {
        transport: Some(HarnessTransport::AppServer),
        kind: TaskFrameKind::Task,
        task_id: "t".to_string(),
        text: "do it".to_string(),
        ts: "2026-07-18T00:00:00.000Z".to_string(),
        correlation_id: None,
        harness: None,
        provider: Some(HarnessProvider::Codex),
        custom_harness: None,
        model: None,
        tool_mode: None,
        workflow: None,
        workflow_fingerprint: None,
        workflow_inputs: Default::default(),
        conversation: None,
        fleet_depth: 0,
    });
    let value: serde_json::Value = serde_json::from_str(&body).expect("valid json");
    assert_eq!(value["transport"], "app_server");

    let decoded = decode_task_frame(&body).expect("decodes");
    assert_eq!(decoded.transport, Some(HarnessTransport::AppServer));
}

/// An ordinary task must be byte-identical to what a peer that predates flavors
/// would send, so the key is dropped when it says nothing.
#[test]
fn omits_the_default_transport_from_the_wire() {
    for transport in [None, Some(HarnessTransport::Cli)] {
        let body = encode_task_frame(EncodeFrameInput {
            transport,
            kind: TaskFrameKind::Task,
            task_id: "t".to_string(),
            text: "do it".to_string(),
            ts: "2026-07-18T00:00:00.000Z".to_string(),
            correlation_id: None,
            harness: None,
            provider: Some(HarnessProvider::Codex),
            custom_harness: None,
            model: None,
            tool_mode: None,
            workflow: None,
            workflow_fingerprint: None,
            workflow_inputs: Default::default(),
            conversation: None,
            fleet_depth: 0,
        });
        let value: serde_json::Value = serde_json::from_str(&body).expect("valid json");
        assert!(value.get("transport").is_none(), "{transport:?}: {body}");
        assert_eq!(decode_task_frame(&body).unwrap().transport, None);
    }
}

/// Failing closed, not open: a worker that ran the CLI because it did not
/// understand the flavor is a silent downgrade the sender cannot detect.
#[test]
fn refuses_a_frame_naming_a_transport_it_does_not_understand() {
    for bad in [json!("quantum"), json!(7), json!({})] {
        let body = json!({
            "proto": MEDULLA_TASK_PROTO,
            "kind": "task",
            "taskId": "t",
            "text": "do it",
            "ts": "2026-07-18T00:00:00.000Z",
            "transport": bad,
        })
        .to_string();
        assert!(decode_task_frame(&body).is_none(), "{bad}");
    }
}
