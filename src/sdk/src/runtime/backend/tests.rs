//! Unit tests for the backend runtime's local fold: event mapping, the
//! optimistic-echo de-duplication, the cycle bracket, log caps, and the
//! session-summary mapper.

use serde_json::{json, Value};

use crate::client::{EventEnvelope as ClientEnvelope, SessionSummary};
use crate::ui::events::TuiEvent;

use super::fold::summary_from_session;
use super::types::{State, Thread, CHAT_CAP, EVENT_CAP};

fn client_env(session: &str, seq: Option<u64>, event: Value) -> ClientEnvelope {
    let mut raw = json!({
        "at": 1234u64,
        "sessionId": session,
        "event": event,
    });
    if let Some(seq) = seq {
        raw["seq"] = json!(seq);
    }
    serde_json::from_value(raw).unwrap()
}

fn state_with_thread() -> State {
    State {
        threads: vec![Thread::new("t1", "main", "sess-1".into())],
        active_id: "t1".into(),
        seq: 0,
        next_thread: 2,
        async_mode: false,
    }
}

#[test]
fn folds_kinds_into_tui_events() {
    let mut s = state_with_thread();
    s.fold(
        "sess-1",
        &client_env("sess-1", Some(1), json!({"kind":"user","body":"hi"})),
    );
    s.fold(
        "sess-1",
        &client_env("sess-1", Some(2), json!({"kind":"assistant","body":"yo"})),
    );
    s.fold(
        "sess-1",
        &client_env(
            "sess-1",
            None,
            json!({"kind":"assistant_delta","delta":"y"}),
        ),
    );
    s.fold(
        "sess-1",
        &client_env(
            "sess-1",
            None,
            json!({"kind":"reasoning_delta","delta":"r"}),
        ),
    );
    s.fold(
        "sess-1",
        &client_env(
            "sess-1",
            None,
            json!({"kind":"tool_call_delta","index":2,"argsDelta":"{"}),
        ),
    );
    s.fold(
        "sess-1",
        &client_env("sess-1", Some(3), json!({"kind":"weird","x":1})),
    );

    let t = &s.threads[0];
    let kinds: Vec<&str> = t.events.iter().map(|e| e.event.kind()).collect();
    assert_eq!(
        kinds,
        vec![
            "user",
            "assistant",
            "assistant_delta",
            "reasoning_delta",
            "tool_call_delta",
            "weird"
        ]
    );
    // Chat events are the user/assistant/error subset only.
    let chat: Vec<&str> = t.chat_events.iter().map(|e| e.event.kind()).collect();
    assert_eq!(chat, vec!["user", "assistant"]);
    // Messages track user + assistant turns.
    assert_eq!(t.messages.len(), 2);
    // The tool-call delta carried its fields through.
    match &t.events[4].event {
        TuiEvent::ToolCallDelta { index, args_delta } => {
            assert_eq!(*index, 2);
            assert_eq!(args_delta, "{");
        }
        other => panic!("expected tool_call_delta, got {other:?}"),
    }
    // Envelope `at` came from the client envelope.
    assert_eq!(t.events[0].at, 1234);
}

#[test]
fn cycle_bracket_drives_running_and_last_result() {
    let mut s = state_with_thread();
    s.threads[0].running = true;
    s.fold(
        "sess-1",
        &client_env(
            "sess-1",
            Some(1),
            json!({"kind":"cycle_start","cycleId":"c1"}),
        ),
    );
    assert!(s.threads[0].running);
    s.fold(
        "sess-1",
        &client_env(
            "sess-1",
            Some(2),
            json!({"kind":"cycle_end","cycleId":"c1","passCount":3,"durationMs":42}),
        ),
    );
    assert!(!s.threads[0].running);
    let lr = s.threads[0].last_result.as_ref().unwrap();
    assert_eq!(lr.pass_count, 3);
}

#[test]
fn optimistic_user_echo_is_deduped() {
    let mut s = state_with_thread();
    s.push_local_user("t1", "hello", 10);
    assert_eq!(s.threads[0].events.len(), 1);
    assert!(s.threads[0].running);
    // The stream echoes the same user turn — it must not duplicate.
    let folded = s.fold(
        "sess-1",
        &client_env("sess-1", Some(1), json!({"kind":"user","body":"hello"})),
    );
    assert!(folded.is_none());
    assert_eq!(s.threads[0].events.len(), 1);
    assert_eq!(s.threads[0].messages.len(), 1);
    // A different user turn is not swallowed.
    let folded = s.fold(
        "sess-1",
        &client_env("sess-1", Some(2), json!({"kind":"user","body":"world"})),
    );
    assert!(folded.is_some());
    assert_eq!(s.threads[0].messages.len(), 2);
}

#[test]
fn event_logs_are_capped() {
    let mut s = state_with_thread();
    for i in 0..(EVENT_CAP + 200) {
        let body = format!("m{i}");
        // Alternate a chatty and a non-chatty event to exercise both caps.
        if i % 2 == 0 {
            s.fold(
                "sess-1",
                &client_env(
                    "sess-1",
                    Some(i as u64),
                    json!({"kind":"assistant","body":body}),
                ),
            );
        } else {
            s.fold(
                "sess-1",
                &client_env(
                    "sess-1",
                    None,
                    json!({"kind":"assistant_delta","delta":body}),
                ),
            );
        }
    }
    let t = &s.threads[0];
    assert_eq!(t.events.len(), EVENT_CAP);
    assert!(t.chat_events.len() <= CHAT_CAP);
}

#[test]
fn session_summary_maps_to_main_chat() {
    let s: SessionSummary = serde_json::from_value(json!({
        "sessionId": "sess-9",
        "title": "Auth refactor",
        "lastActiveAt": 1_700_000_000_000i64,
        "status": "active",
        "lastSeq": 7,
    }))
    .unwrap();
    let row = summary_from_session(&s);
    assert_eq!(row.session_id, "sess-9");
    assert_eq!(row.name, "Auth refactor");
    assert_eq!(row.turns, 3); // 7 / 2
    assert_eq!(row.thread_count, 1);
    assert_eq!(row.updated_at, "2023-11-14T22:13:20.000Z");
}

#[test]
fn session_summary_falls_back_to_id_for_name() {
    let s: SessionSummary = serde_json::from_value(json!({
        "sessionId": "sess-bare",
        "status": "idle",
    }))
    .unwrap();
    let row = summary_from_session(&s);
    assert_eq!(row.name, "sess-bare");
    assert_eq!(row.turns, 0);
}
