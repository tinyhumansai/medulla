//! Work-snapshot cases for task-frame encoding and tolerant decoding.

use crate::protocol::{decode_task_frame, EncodeFrameInput, HarnessProvider, TaskFrameKind};
use serde_json::json;

#[test]
fn a_frame_carries_the_workers_work_snapshot_across_the_wire() {
    use crate::harness_work::{kinds, WorkFold};

    let mut fold = WorkFold::new();
    fold.apply(
        kinds::TODO_UPDATE,
        &json!({ "todos": [
            { "content": "read the code", "status": "completed" },
            { "content": "write the fold", "status": "in_progress" },
        ]}),
        1,
    );
    fold.apply(
        kinds::SUBAGENT_START,
        &json!({ "call_id": "t1", "description": "review it" }),
        2,
    );
    let snapshot = fold.into_snapshot();

    let body = crate::protocol::encode_task_frame_with_work(
        EncodeFrameInput {
            kind: TaskFrameKind::Status,
            task_id: "cycle-1".to_string(),
            text: "write the fold · todo 1/2".to_string(),
            ts: "2026-07-18T00:00:00.000Z".to_string(),
            correlation_id: None,
            harness: Some(HarnessProvider::Claude),
            provider: None,
            custom_harness: None,
            model: None,
            tool_mode: None,
            workflow: None,
            workflow_fingerprint: None,
            workflow_inputs: Default::default(),
            conversation: None,
            fleet_depth: 0,
        },
        None,
        Some(snapshot.clone()),
    );
    let decoded = decode_task_frame(&body).expect("the frame decodes");
    assert_eq!(decoded.work.as_deref(), Some(&snapshot));
}

#[test]
fn an_empty_work_snapshot_is_left_off_the_wire() {
    let body = crate::protocol::encode_task_frame_with_work(
        EncodeFrameInput {
            kind: TaskFrameKind::Status,
            task_id: "cycle-1".to_string(),
            text: "thinking".to_string(),
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
        },
        None,
        Some(crate::harness_work::WorkSnapshot::default()),
    );
    let value: serde_json::Value = serde_json::from_str(&body).unwrap();
    assert!(
        value.get("work").is_none(),
        "an empty snapshot costs bytes for nothing"
    );
}

#[test]
fn a_malformed_work_snapshot_does_not_sink_the_frame() {
    let body = json!({
        "proto": crate::protocol::MEDULLA_TASK_PROTO,
        "kind": "reply",
        "taskId": "cycle-1",
        "text": "done",
        "ts": "2026-07-18T00:00:00.000Z",
        "work": "not an object",
    })
    .to_string();
    let decoded = decode_task_frame(&body).expect("the frame still decodes");
    assert_eq!(decoded.text, "done");
    assert!(decoded.work.is_none());
}
