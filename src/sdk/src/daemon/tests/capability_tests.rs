//! Capability-probe caching, status-frame throttling, and the pure
//! semantic-event → status-line mapping ([`status_detail`]).

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use serde_json::json;

use crate::daemon::{status_detail, DaemonRuntime, NowFn};
use crate::tinyplace::{HarnessEvent, TaskFrameKind};

use super::{
    base_config, capabilities_frame, counting_capability_runner, decoded_frames, recording_send,
    status_runner, task_frame, tool_call_event,
};

#[tokio::test]
async fn throttles_status_frames() {
    let run_task = status_runner(3);
    let (send, recorded) = recording_send();
    let runtime = DaemonRuntime::new(base_config(), run_task, send);

    // now() sequence: first event passes (10000 - MIN ≥ throttle), the next two
    // fall inside the 4s window relative to 10000, so only one status is emitted.
    let seq = Arc::new(vec![10_000i64, 11_000, 12_000]);
    let index = Arc::new(AtomicUsize::new(0));
    let now: NowFn = Arc::new(move || {
        let position = index.fetch_add(1, Ordering::SeqCst);
        *seq.get(position).unwrap_or(seq.last().unwrap())
    });
    let runtime = runtime.with_now(now);

    runtime.handle_message(
        "peer".into(),
        String::new(),
        Some(task_frame("t1", "work", None)),
    );
    runtime.idle().await;

    let frames = decoded_frames(&recorded);
    let status_count = frames
        .iter()
        .filter(|f| f.kind == TaskFrameKind::Status)
        .count();
    assert_eq!(
        status_count, 1,
        "exactly one status frame should survive throttling"
    );
    assert!(frames
        .iter()
        .any(|f| f.kind == TaskFrameKind::Reply && f.text == "ok"));
}

#[tokio::test]
async fn capabilities_probe_is_cached_across_askers() {
    let count = Arc::new(AtomicUsize::new(0));
    let run_task = counting_capability_runner(count.clone());
    let (send, recorded) = recording_send();
    let runtime = DaemonRuntime::new(base_config(), run_task, send);

    runtime.handle_message(
        "peer".into(),
        String::new(),
        Some(capabilities_frame("c1", None)),
    );
    runtime.idle().await;
    runtime.handle_message(
        "peer".into(),
        String::new(),
        Some(capabilities_frame("c2", None)),
    );
    runtime.idle().await;

    // Two result frames, but the underlying probe ran exactly once (cached).
    let frames = decoded_frames(&recorded);
    let results = frames
        .iter()
        .filter(|f| f.kind == TaskFrameKind::CapabilitiesResult)
        .count();
    assert_eq!(results, 2, "each asker gets a capabilities_result");
    assert_eq!(
        count.load(Ordering::SeqCst),
        1,
        "probe cached after first run"
    );
}

#[tokio::test]
async fn status_detail_maps_event_kinds() {
    let tool_call = tool_call_event().event;
    assert_eq!(
        status_detail(&tool_call).as_deref(),
        Some("running Bash: ls -la")
    );

    let thinking = HarnessEvent {
        kind: "agent_thinking".to_string(),
        role: "agent".to_string(),
        payload: json!({ "text": "hmm" }),
        ..Default::default()
    };
    assert_eq!(status_detail(&thinking).as_deref(), Some("thinking"));

    let message = HarnessEvent {
        kind: "agent_message".to_string(),
        payload: json!({ "text": "done" }),
        ..Default::default()
    };
    assert_eq!(status_detail(&message).as_deref(), Some("writing response"));

    let failed_tool = HarnessEvent {
        kind: "tool_result".to_string(),
        payload: json!({ "call_id": "c", "ok": false, "is_error": true, "output": "", "output_bytes": 0 }),
        ..Default::default()
    };
    assert_eq!(status_detail(&failed_tool).as_deref(), Some("tool failed"));

    let ok_tool = HarnessEvent {
        kind: "tool_result".to_string(),
        payload: json!({ "call_id": "c", "ok": true, "is_error": false, "output": "", "output_bytes": 0 }),
        ..Default::default()
    };
    assert_eq!(status_detail(&ok_tool).as_deref(), Some("tool completed"));

    // Status: a non-empty detail wins over the state.
    let status_detailed = HarnessEvent {
        kind: "status".to_string(),
        payload: json!({ "state": "running", "detail": "compiling" }),
        ..Default::default()
    };
    assert_eq!(
        status_detail(&status_detailed).as_deref(),
        Some("compiling")
    );

    // Status: an empty detail falls back to the state string.
    let status_state = HarnessEvent {
        kind: "status".to_string(),
        payload: json!({ "state": "running", "detail": "" }),
        ..Default::default()
    };
    assert_eq!(status_detail(&status_state).as_deref(), Some("running"));

    // Status: both empty yields nothing.
    let status_blank = HarnessEvent {
        kind: "status".to_string(),
        payload: json!({ "state": "", "detail": "" }),
        ..Default::default()
    };
    assert_eq!(status_detail(&status_blank), None);

    // Error: capped and prefixed. 300 chars exceeds the 200-char cap.
    let error = HarnessEvent {
        kind: "error".to_string(),
        payload: json!({ "message": "x".repeat(300) }),
        ..Default::default()
    };
    let detail = status_detail(&error).expect("error maps to a detail");
    assert!(detail.starts_with("error: x"));
    assert_eq!(detail.chars().count(), 200);

    // An event kind with no status projection returns None.
    let lifecycle = HarnessEvent {
        kind: "lifecycle".to_string(),
        payload: json!({ "phase": "session_start" }),
        ..Default::default()
    };
    assert_eq!(status_detail(&lifecycle), None);
}
