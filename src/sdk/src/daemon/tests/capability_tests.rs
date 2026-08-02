//! Capability-probe caching, status-frame throttling, and the pure
//! semantic-event → status-line mapping ([`status_detail`]).

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use serde_json::json;

use crate::daemon::{status_detail, work_detail, DaemonRuntime, NowFn};
use crate::tinyplace::{AgentCapabilities, HarnessEvent, TaskFrameKind};

use super::{
    base_config, capabilities_frame, chatter_status_runner, counting_capability_runner,
    decoded_frames, quick_thinking_runner, quick_tool_runner, recording_send, status_runner,
    task_frame, tool_call_event,
};

#[tokio::test]
async fn throttles_status_frames() {
    let run_task = chatter_status_runner(3);
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
async fn flushes_final_thinking_snapshot_after_throttling() {
    let (send, recorded) = recording_send();
    let runtime = DaemonRuntime::new(base_config(), quick_thinking_runner(), send);
    let seq = Arc::new(vec![10_000i64, 11_000]);
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

    let statuses = decoded_frames(&recorded)
        .into_iter()
        .filter(|frame| frame.kind == TaskFrameKind::Status)
        .map(|frame| frame.text)
        .collect::<Vec<_>>();
    assert_eq!(
        statuses,
        [
            "thinking · checking",
            "thinking · checking the final result"
        ]
    );
}

#[tokio::test]
async fn tool_settlements_bypass_status_throttling() {
    let (send, recorded) = recording_send();
    let runtime = DaemonRuntime::new(base_config(), quick_tool_runner(), send);

    // The result lands only 1s after the call, inside the default 4s window.
    let seq = Arc::new(vec![10_000i64, 11_000]);
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

    let statuses = decoded_frames(&recorded)
        .into_iter()
        .filter(|frame| frame.kind == TaskFrameKind::Status)
        .map(|frame| frame.text)
        .collect::<Vec<_>>();
    assert_eq!(statuses.len(), 2, "{statuses:?}");
    assert!(statuses[0].starts_with("running Bash"), "{statuses:?}");
    assert!(statuses[1].starts_with("tool completed"), "{statuses:?}");
}

#[tokio::test]
async fn tool_call_details_bypass_status_throttling() {
    let run_task = status_runner(2);
    let (send, recorded) = recording_send();
    let runtime = DaemonRuntime::new(base_config(), run_task, send);

    // The second call represents ACP enriching the same call with raw input.
    // Both must reach the copilot even though they land inside the 4s window.
    let seq = Arc::new(vec![10_000i64, 11_000]);
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

    let statuses = decoded_frames(&recorded)
        .into_iter()
        .filter(|frame| frame.kind == TaskFrameKind::Status)
        .collect::<Vec<_>>();
    assert_eq!(statuses.len(), 2, "{statuses:?}");
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
async fn capabilities_advertise_only_custom_harnesses_with_available_keys() {
    let count = Arc::new(AtomicUsize::new(0));
    let run_task = counting_capability_runner(count);
    let (send, recorded) = recording_send();
    let mut config = base_config();
    let ready = crate::config::CustomHarnessConfig::from_editor_line(
        "ready | Ready | claude | openrouter/ready | | this-device",
    )
    .unwrap();
    let mut unavailable = crate::config::CustomHarnessConfig::from_editor_line(
        "missing | Missing | claude | openrouter/missing | | this-device",
    )
    .unwrap();
    unavailable.api_key_env = "MISSING_OPENROUTER_KEY".into();
    let unavailable_provider = crate::config::CustomHarnessConfig::from_editor_line(
        "codex | Codex | codex | openrouter/codex | | this-device",
    )
    .unwrap();
    config
        .env
        .insert(ready.api_key_env.clone(), "configured".into());
    config.custom_harnesses = vec![ready, unavailable, unavailable_provider];
    let runtime = DaemonRuntime::new(config, run_task, send);

    runtime.handle_message(
        "peer".into(),
        String::new(),
        Some(capabilities_frame("custom-capabilities", None)),
    );
    runtime.idle().await;

    let result = decoded_frames(&recorded)
        .into_iter()
        .find(|frame| frame.kind == TaskFrameKind::CapabilitiesResult)
        .expect("capabilities result");
    let capabilities: AgentCapabilities = serde_json::from_str(&result.text).unwrap();
    let advertised: Vec<_> = capabilities
        .custom_harnesses
        .iter()
        .map(|harness| harness.id.as_str())
        .collect();
    assert_eq!(advertised, vec!["ready"]);
}

#[tokio::test]
async fn status_detail_maps_event_kinds() {
    let tool_call = tool_call_event().event;
    assert_eq!(
        status_detail(&tool_call).as_deref(),
        Some("running Bash: ls -la\u{1f}c1")
    );
    let terminal = HarnessEvent {
        kind: "tool_call".to_string(),
        payload: json!({
            "call_id": "c2",
            "tool_name": "execute",
            "tool_kind": "shell",
            "display": "Terminal",
            "input": { "command": "cargo test --workspace\ncargo clippy" }
        }),
        ..Default::default()
    };
    assert_eq!(
        status_detail(&terminal).as_deref(),
        Some("running Terminal · $ cargo test --workspace cargo clippy\u{1f}c2")
    );
    let terminal_without_input = HarnessEvent {
        kind: "tool_call".to_string(),
        payload: json!({
            "call_id": "c2-pending",
            "tool_name": "execute",
            "tool_kind": "shell",
            "display": "Terminal"
        }),
        ..Default::default()
    };
    assert_eq!(
        status_detail(&terminal_without_input).as_deref(),
        Some("running Terminal\u{1f}c2-pending")
    );
    let secret_command = HarnessEvent {
        kind: "tool_call".to_string(),
        payload: json!({
            "call_id": "c3",
            "tool_name": "execute",
            "tool_kind": "shell",
            "input": {
                "command": "curl -H 'Authorization: Bearer top-secret' https://example.test"
            }
        }),
        ..Default::default()
    };
    let command_detail = status_detail(&secret_command).expect("secret command has safe status");
    assert_eq!(
        command_detail,
        "running Terminal · $ [credential redacted]\u{1f}c3"
    );
    assert!(!command_detail.contains("top-secret"));

    let string_command = HarnessEvent {
        kind: "tool_call".to_string(),
        payload: json!({
            "call_id": "c3-string",
            "tool_name": "shell",
            "tool_kind": "shell",
            "display": "curl -H 'Authorization: Bearer string-secret' https://example.test",
            "input": "curl -H 'Authorization: Bearer string-secret' https://example.test"
        }),
        ..Default::default()
    };
    let string_detail =
        status_detail(&string_command).expect("string command input has safe status");
    assert_eq!(
        string_detail,
        "running Shell · $ [credential redacted]\u{1f}c3-string"
    );
    assert!(!string_detail.contains("string-secret"));

    let secret_url = HarnessEvent {
        kind: "tool_call".to_string(),
        payload: json!({
            "call_id": "c4",
            "tool_name": "fetch",
            "input": {
                "url": "https://operator:password@example.test/hook?token=top-secret"
            }
        }),
        ..Default::default()
    };
    let url_detail = status_detail(&secret_url).expect("secret URL has safe status");
    assert_eq!(
        url_detail,
        "running Fetch · [credential redacted URL]\u{1f}c4"
    );
    assert!(!url_detail.contains("operator"));
    assert!(!url_detail.contains("top-secret"));

    let thinking = HarnessEvent {
        kind: "agent_thinking".to_string(),
        role: "agent".to_string(),
        payload: json!({ "text": "hmm" }),
        ..Default::default()
    };
    assert_eq!(status_detail(&thinking).as_deref(), Some("thinking · hmm"));
    let empty_thinking = HarnessEvent {
        payload: json!({ "text": " \n\t" }),
        ..thinking.clone()
    };
    assert_eq!(status_detail(&empty_thinking).as_deref(), Some("thinking"));
    let secret_thinking = HarnessEvent {
        payload: json!({ "text": "use sk-abcdefghijklmnop0123456789 now" }),
        ..thinking.clone()
    };
    assert_eq!(
        status_detail(&secret_thinking).as_deref(),
        Some("thinking · use [REDACTED] now")
    );
    let basic_auth_thinking = HarnessEvent {
        payload: serde_json::json!({
            "text": "trying Authorization: Basic ZGJ1c2VyOmRicGFzcw=="
        }),
        ..thinking.clone()
    };
    assert_eq!(
        status_detail(&basic_auth_thinking).as_deref(),
        Some("thinking · [credential redacted]")
    );
    let userinfo_thinking = HarnessEvent {
        payload: serde_json::json!({
            "text": "connecting to postgres://dbuser:dbpass@host/db"
        }),
        ..thinking.clone()
    };
    assert_eq!(
        status_detail(&userinfo_thinking).as_deref(),
        Some("thinking · [credential redacted]")
    );

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
    assert_eq!(
        status_detail(&failed_tool).as_deref(),
        Some("tool failed\u{1f}c")
    );

    let ok_tool = HarnessEvent {
        kind: "tool_result".to_string(),
        payload: json!({ "call_id": "c", "ok": true, "is_error": false, "output": "", "output_bytes": 0 }),
        ..Default::default()
    };
    assert_eq!(
        status_detail(&ok_tool).as_deref(),
        Some("tool completed\u{1f}c")
    );

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

#[test]
fn work_detail_describes_events_the_published_vocabulary_cannot() {
    use crate::harness_work::{kinds, WorkFold};

    // A todo write decodes to nothing in `status_detail` — the event union
    // predates it — so without a work-derived line a harness could rewrite its
    // whole plan and the peer would see no status change at all.
    let mut fold = WorkFold::new();
    fold.apply(
        kinds::TODO_UPDATE,
        &json!({ "todos": [
            { "content": "a", "status": "completed" },
            { "content": "b", "status": "in_progress", "activeForm": "Doing b" },
        ]}),
        1,
    );
    assert_eq!(
        work_detail(fold.snapshot()).as_deref(),
        Some("Doing b · todo 1/2")
    );

    // With nothing in progress, a running sub-agent is the next best answer.
    let mut delegating = WorkFold::new();
    delegating.apply(
        kinds::SUBAGENT_START,
        &json!({ "call_id": "s1", "description": "review" }),
        1,
    );
    assert_eq!(
        work_detail(delegating.snapshot()).as_deref(),
        Some("1 sub-agent running")
    );

    assert_eq!(work_detail(&Default::default()), None);
}
