//! Unit tests for the `medulla-task/1` frame codec's core encode/decode
//! round-trips: kinds, optional fields, workflow markers, and malformed input.
//!
//! Split from a single oversized file (see AGENTS.md's 500-line ceiling):
//! capability/budget/readiness wire tests live in `capabilities.rs`, and
//! session/transport-attachment tests live in `attachments.rs`.

use crate::protocol::{
    decode_task_frame, encode_task_frame, encode_workflow_authoring_task_frame,
    encode_workflow_node_task_frame, EncodeFrameInput, HarnessProvider, TaskFrameKind,
    MEDULLA_TASK_PROTO,
};
use serde_json::json;

#[test]
fn encodes_a_minimal_frame() {
    let body = encode_task_frame(EncodeFrameInput {
        transport: None,
        kind: TaskFrameKind::Task,
        task_id: "cycle-1".to_string(),
        text: "do the thing".to_string(),
        ts: "2026-07-18T00:00:00.000Z".to_string(),
        correlation_id: None,
        harness: None,
        provider: None,
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
    assert_eq!(value["proto"], MEDULLA_TASK_PROTO);
    assert_eq!(value["kind"], "task");
    assert_eq!(value["taskId"], "cycle-1");
    assert_eq!(value["text"], "do the thing");
    assert_eq!(value["ts"], "2026-07-18T00:00:00.000Z");
    // Optional fields are omitted when absent.
    assert!(value.get("correlationId").is_none());
    assert!(value.get("harness").is_none());
    assert!(value.get("provider").is_none());
    assert!(value.get("model").is_none());
}

/// A workflow agent instruction is distinct from a frame that starts a saved
/// workflow, but its authority marker must survive the runner/daemon boundary.
#[test]
fn workflow_node_marker_round_trips() {
    let body = encode_workflow_node_task_frame(EncodeFrameInput {
        transport: None,
        kind: TaskFrameKind::Task,
        task_id: "wf:run:agent#1".to_string(),
        text: "inspect the checkout".to_string(),
        ts: "2026-08-08T00:00:00.000Z".to_string(),
        correlation_id: None,
        harness: None,
        provider: Some(HarnessProvider::Openhuman),
        custom_harness: None,
        model: None,
        tool_mode: None,
        workflow: None,
        workflow_fingerprint: None,
        workflow_inputs: Default::default(),
        conversation: None,
        fleet_depth: 0,
    });

    let value: serde_json::Value = serde_json::from_str(&body).expect("frame JSON");
    assert_eq!(value["workflowNode"], true);
    assert!(
        decode_task_frame(&body)
            .expect("workflow-node frame decodes")
            .workflow_node
    );
}

/// The copilot's authoring turn is workflow-plane work like a node, and says so
/// — but it carries the extra marker that keeps the workflow tools with it. A
/// node frame must not carry it, or the withholding it enables goes away.
#[test]
fn the_authoring_marker_round_trips_and_is_absent_from_a_node_frame() {
    let input = || EncodeFrameInput {
        transport: None,
        kind: TaskFrameKind::Task,
        task_id: "copilot-1".to_string(),
        text: "add a lint step".to_string(),
        ts: "2026-08-28T00:00:00.000Z".to_string(),
        correlation_id: None,
        harness: None,
        provider: Some(HarnessProvider::Openhuman),
        custom_harness: None,
        model: None,
        tool_mode: None,
        workflow: None,
        workflow_fingerprint: None,
        workflow_inputs: Default::default(),
        conversation: Some("workflows-pane".to_string()),
        fleet_depth: 0,
    };

    let body = encode_workflow_authoring_task_frame(input());
    let value: serde_json::Value = serde_json::from_str(&body).expect("frame JSON");
    assert_eq!(value["workflowNode"], true, "still workflow-plane work");
    assert_eq!(value["workflowAuthoring"], true);
    let decoded = decode_task_frame(&body).expect("authoring frame decodes");
    assert!(decoded.workflow_node && decoded.workflow_authoring);

    // A graph step says nothing of the sort, on the wire or after decoding.
    let node = encode_workflow_node_task_frame(input());
    let value: serde_json::Value = serde_json::from_str(&node).expect("frame JSON");
    assert!(
        value.get("workflowAuthoring").is_none(),
        "an agent node must not claim to be an authoring turn: {value}"
    );
    assert!(
        !decode_task_frame(&node)
            .expect("decodes")
            .workflow_authoring
    );
}

#[test]
fn rejects_a_fleet_depth_that_cannot_fit_the_protocol_type() {
    let body = json!({
        "proto": MEDULLA_TASK_PROTO,
        "kind": "task",
        "taskId": "cycle-1",
        "text": "do the thing",
        "ts": "2026-07-18T00:00:00.000Z",
        "fleet_depth": 256,
    })
    .to_string();

    assert!(decode_task_frame(&body).is_none());
}

#[test]
fn encodes_optional_fields_when_present() {
    let body = encode_task_frame(EncodeFrameInput {
        transport: None,
        kind: TaskFrameKind::CapabilitiesResult,
        task_id: "t".to_string(),
        text: "{}".to_string(),
        ts: "2026-07-18T00:00:00.000Z".to_string(),
        correlation_id: Some("corr-9".to_string()),
        harness: Some(HarnessProvider::Codex),
        provider: Some(HarnessProvider::Claude),
        custom_harness: Some("deepseek-claude".into()),
        model: Some("anthropic/claude-opus-4.8".to_string()),
        tool_mode: None,
        workflow: Some("nightly-sweep".to_string()),
        workflow_fingerprint: Some("nightly-fingerprint".to_string()),
        workflow_inputs: json!({ "repo": "acme/api", "depth": 3 })
            .as_object()
            .unwrap()
            .clone(),
        conversation: None,
        fleet_depth: 0,
    });
    let value: serde_json::Value = serde_json::from_str(&body).unwrap();
    assert_eq!(value["kind"], "capabilities_result");
    assert_eq!(value["correlationId"], "corr-9");
    assert_eq!(value["harness"], "codex");
    assert_eq!(value["provider"], "claude");
    assert_eq!(value["model"], "anthropic/claude-opus-4.8");
    assert_eq!(value["workflow"], "nightly-sweep");
    assert_eq!(value["workflowFingerprint"], "nightly-fingerprint");
    assert_eq!(value["inputs"]["repo"], "acme/api");
    let decoded = decode_task_frame(&body).expect("the full frame decodes");
    assert_eq!(
        decoded.workflow_fingerprint.as_deref(),
        Some("nightly-fingerprint")
    );
    assert_eq!(decoded.workflow_inputs["depth"], json!(3));
}

#[test]
fn round_trips_every_kind() {
    for (kind, wire) in [
        (TaskFrameKind::Task, "task"),
        (TaskFrameKind::Input, "input"),
        (TaskFrameKind::Status, "status"),
        (TaskFrameKind::Reply, "reply"),
        (TaskFrameKind::Error, "error"),
        (TaskFrameKind::Ack, "ack"),
        (TaskFrameKind::Capabilities, "capabilities"),
        (TaskFrameKind::CapabilitiesResult, "capabilities_result"),
        (TaskFrameKind::SystemInfo, "system_info"),
        (TaskFrameKind::SystemInfoResult, "system_info_result"),
    ] {
        let body = encode_task_frame(EncodeFrameInput {
            transport: None,
            kind,
            task_id: "t".to_string(),
            text: "x".to_string(),
            ts: "ts".to_string(),
            correlation_id: None,
            harness: None,
            provider: None,
            custom_harness: None,
            model: None,
            tool_mode: None,
            workflow: None,
            workflow_fingerprint: None,
            workflow_inputs: Default::default(),
            conversation: None,
            fleet_depth: 0,
        });
        let decoded = decode_task_frame(&body).expect("valid frame decodes");
        assert_eq!(decoded.kind, kind);
        assert_eq!(decoded.kind.as_str(), wire);
    }
}

#[test]
fn decodes_a_full_frame() {
    let body = json!({
        "proto": MEDULLA_TASK_PROTO,
        "kind": "reply",
        "taskId": "cycle-7",
        "text": "done",
        "ts": "2026-07-18T00:00:00.000Z",
        "correlationId": "corr-1",
        "harness": "opencode",
        "provider": "claude",
    })
    .to_string();
    let frame = decode_task_frame(&body).unwrap();
    assert_eq!(frame.kind, TaskFrameKind::Reply);
    assert_eq!(frame.task_id, "cycle-7");
    assert_eq!(frame.correlation_id.as_deref(), Some("corr-1"));
    assert_eq!(frame.harness, Some(HarnessProvider::Opencode));
    assert_eq!(frame.provider, Some(HarnessProvider::Claude));
}

#[test]
fn carries_a_model_hint_through_encode_and_decode() {
    let body = encode_task_frame(EncodeFrameInput {
        transport: None,
        kind: TaskFrameKind::Task,
        task_id: "t".to_string(),
        text: "x".to_string(),
        ts: "ts".to_string(),
        correlation_id: None,
        harness: None,
        provider: None,
        custom_harness: None,
        model: Some("openrouter/some-model".to_string()),
        tool_mode: None,
        workflow: None,
        workflow_fingerprint: None,
        workflow_inputs: Default::default(),
        conversation: None,
        fleet_depth: 0,
    });
    let decoded = decode_task_frame(&body).unwrap();
    assert_eq!(decoded.model.as_deref(), Some("openrouter/some-model"));
}

#[test]
fn decode_treats_absent_or_blank_model_as_none() {
    // Absent entirely.
    let absent = json!({
        "proto": MEDULLA_TASK_PROTO, "kind": "task", "taskId": "t", "text": "x", "ts": "ts",
    })
    .to_string();
    assert_eq!(decode_task_frame(&absent).unwrap().model, None);
    // Present but blank — treated as no hint so the daemon keeps its default.
    let blank = json!({
        "proto": MEDULLA_TASK_PROTO, "kind": "task", "taskId": "t", "text": "x", "ts": "ts",
        "model": "   ",
    })
    .to_string();
    assert_eq!(decode_task_frame(&blank).unwrap().model, None);
}

#[test]
fn decode_tolerates_missing_ts() {
    let body = json!({
        "proto": MEDULLA_TASK_PROTO,
        "kind": "ack",
        "taskId": "t",
        "text": "",
    })
    .to_string();
    let frame = decode_task_frame(&body).unwrap();
    assert_eq!(frame.ts, "");
}

#[test]
fn decode_drops_unknown_provider_without_failing() {
    let body = json!({
        "proto": MEDULLA_TASK_PROTO,
        "kind": "task",
        "taskId": "t",
        "text": "x",
        "ts": "ts",
        "provider": "gemini",
    })
    .to_string();
    let frame = decode_task_frame(&body).unwrap();
    assert_eq!(frame.provider, None);
}

#[test]
fn decode_rejects_non_frames() {
    assert!(decode_task_frame("not json").is_none());
    assert!(decode_task_frame("42").is_none());
    assert!(decode_task_frame(r#"{"hello":"world"}"#).is_none());
    // Wrong proto tag.
    assert!(
        decode_task_frame(r#"{"proto":"other/1","kind":"task","taskId":"t","text":"x"}"#).is_none()
    );
    // Unknown kind.
    assert!(decode_task_frame(
        &json!({"proto": MEDULLA_TASK_PROTO, "kind": "nope", "taskId": "t", "text": "x"})
            .to_string()
    )
    .is_none());
    // Missing required text.
    assert!(decode_task_frame(
        &json!({"proto": MEDULLA_TASK_PROTO, "kind": "task", "taskId": "t"}).to_string()
    )
    .is_none());
}
