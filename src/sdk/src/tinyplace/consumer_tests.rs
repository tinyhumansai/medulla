//! Tests for the consumer module.

use crate::tinyplace::{
    apply_session_envelope, fold_session_envelopes, initial_session_view, parse_session_envelope,
    AnySessionEnvelope, SessionViewLimits, DEFAULT_LIMITS, SESSION_ENVELOPE_VERSION_V2,
    STATE_ERRORED, STATE_IDLE, STATE_RUNNING, STATE_RUNNING_TOOL, STATE_STOPPED,
    STATE_WAITING_APPROVAL,
};
use serde_json::{json, Value};

/// Build a v2 envelope with the given `event` object (kind/role/payload merged
/// with id/seq/ts). Envelopes come from the SDK via `parse_session_envelope`.
fn v2_envelope(seq: i64, event_body: Value) -> AnySessionEnvelope {
    let mut event = json!({ "id": format!("id-{seq}"), "seq": seq, "ts": format!("ts-{seq}") });
    let obj = event.as_object_mut().unwrap();
    for (k, val) in event_body.as_object().unwrap() {
        obj.insert(k.clone(), val.clone());
    }
    let body = json!({
        "envelope_version": SESSION_ENVELOPE_VERSION_V2,
        "version": 2,
        "bucket": { "unit": "minute", "start": "s", "end": "e" },
        "scope": {
            "type": "session", "key": "k", "cwd": "/repo",
            "wrapper_session_id": "wsid-1", "harness_session_id": "hsid-1",
        },
        "harness": { "provider": "claude", "command": "claude", "argv": ["-p"] },
        "event": event,
        "source": { "path": "/t.jsonl", "record_type": "jsonl" },
    })
    .to_string();
    parse_session_envelope(&body).unwrap()
}

fn tool_call(seq: i64, call_id: &str, name: &str, display: &str) -> AnySessionEnvelope {
    v2_envelope(
        seq,
        json!({"kind":"tool_call","role":"agent","payload":{
        "call_id":call_id,"tool_name":name,"tool_kind":"shell","display":display,"input":{}}}),
    )
}

fn tool_result(seq: i64, call_id: &str, is_error: bool) -> AnySessionEnvelope {
    v2_envelope(
        seq,
        json!({"kind":"tool_result","role":"agent","payload":{
        "call_id":call_id,"ok":!is_error,"is_error":is_error,"output":"o","output_bytes":1}}),
    )
}

#[test]
fn initial_view_is_idle() {
    let view = initial_session_view();
    assert_eq!(view.status, STATE_IDLE);
    assert_eq!(view.current_task, "idle");
    assert_eq!(view.last_seq, -1);
    assert!(view.tools.is_empty());
    assert!(view.feed.is_empty());
}

#[test]
fn applies_a_tool_call_then_result() {
    let mut view = initial_session_view();
    assert!(apply_session_envelope(
        &mut view,
        &tool_call(0, "c1", "Bash", "npm test"),
        DEFAULT_LIMITS
    ));
    assert_eq!(view.provider.as_deref(), Some("claude"));
    assert_eq!(view.cwd.as_deref(), Some("/repo"));
    assert_eq!(view.status, STATE_RUNNING_TOOL);
    assert_eq!(view.current_task, "Bash: npm test");
    assert_eq!(view.last_seq, 0);
    assert_eq!(view.tools.len(), 1);
    assert!(!view.tools[0].done);
    assert_eq!(view.feed.len(), 1);
    assert_eq!(view.feed[0].text, "npm test");

    assert!(apply_session_envelope(
        &mut view,
        &tool_result(1, "c1", false),
        DEFAULT_LIMITS
    ));
    assert!(view.tools[0].done);
    assert_eq!(view.tools[0].ok, Some(true));
    assert_eq!(view.tools[0].is_error, Some(false));
    assert_eq!(view.tools[0].output_bytes, Some(1));
    assert_eq!(view.feed.last().unwrap().text, "ok");
}

#[test]
fn out_of_order_and_duplicate_packets_are_ignored() {
    let mut view = initial_session_view();
    assert!(apply_session_envelope(
        &mut view,
        &tool_call(5, "c1", "Bash", "x"),
        DEFAULT_LIMITS
    ));
    // Same seq — duplicate.
    assert!(!apply_session_envelope(
        &mut view,
        &tool_call(5, "c2", "Bash", "y"),
        DEFAULT_LIMITS
    ));
    // Lower seq — out of order.
    assert!(!apply_session_envelope(
        &mut view,
        &tool_call(3, "c3", "Bash", "z"),
        DEFAULT_LIMITS
    ));
    assert_eq!(view.last_seq, 5);
    assert_eq!(view.tools.len(), 1);
}

#[test]
fn v1_envelopes_are_ignored() {
    let mut view = initial_session_view();
    let body = json!({
        "envelope_version": "tinyplace.harness.session.v1",
        "version": 1,
        "bucket": { "unit": "hour", "start": "s", "end": "e" },
        "scope": { "type": "folder", "key": "k", "cwd": "/r",
            "wrapper_session_id": "w", "harness_session_id": "h" },
        "harness": { "provider": "codex", "command": "codex", "argv": [] },
        "message": { "id": "m", "line": 1, "role": "agent", "text": "hi", "timestamp": "t" },
        "source": { "path": "/p", "record_type": "x" },
    })
    .to_string();
    let env = parse_session_envelope(&body).unwrap();
    assert!(matches!(env, AnySessionEnvelope::V1(_)));
    assert!(!apply_session_envelope(&mut view, &env, DEFAULT_LIMITS));
    assert_eq!(view.last_seq, -1);
}

#[test]
fn status_and_approval_and_error_update_state() {
    let mut view = initial_session_view();
    apply_session_envelope(
        &mut view,
        &v2_envelope(
            0,
            json!({"kind":"status","role":"agent","payload":{"state":"running","detail":"thinking"}}),
        ),
        DEFAULT_LIMITS,
    );
    assert_eq!(view.status, STATE_RUNNING);
    assert_eq!(view.current_task, "thinking");

    apply_session_envelope(
        &mut view,
        &v2_envelope(
            1,
            json!({"kind":"approval_request","role":"agent","payload":{"tool_name":"Bash","display":"rm -rf"}}),
        ),
        DEFAULT_LIMITS,
    );
    assert_eq!(view.status, STATE_WAITING_APPROVAL);
    assert_eq!(view.current_task, "approval: rm -rf");

    apply_session_envelope(
        &mut view,
        &v2_envelope(
            2,
            json!({"kind":"error","role":"agent","payload":{"message":"fatal boom","fatal":true}}),
        ),
        DEFAULT_LIMITS,
    );
    assert_eq!(view.status, STATE_ERRORED);

    apply_session_envelope(
        &mut view,
        &v2_envelope(
            3,
            json!({"kind":"lifecycle","role":"agent","payload":{"phase":"session_end"}}),
        ),
        DEFAULT_LIMITS,
    );
    assert_eq!(view.status, STATE_STOPPED);
    assert_eq!(view.current_task, "stopped");
}

#[test]
fn caps_tools_and_feed() {
    let limits = SessionViewLimits { tools: 3, feed: 4 };
    let mut view = initial_session_view();
    for seq in 0..10 {
        apply_session_envelope(
            &mut view,
            &tool_call(seq, &format!("c{seq}"), "Bash", &format!("cmd-{seq}")),
            limits,
        );
    }
    assert_eq!(view.tools.len(), 3);
    // Newest last: the final three calls survive.
    assert_eq!(view.tools[0].display, "cmd-7");
    assert_eq!(view.tools[2].display, "cmd-9");
    assert_eq!(view.feed.len(), 4);
    assert_eq!(view.feed[3].text, "cmd-9");
}

#[test]
fn user_prompt_feed_entry_is_owner_role() {
    let mut view = initial_session_view();
    apply_session_envelope(
        &mut view,
        &v2_envelope(
            0,
            json!({"kind":"user_prompt","role":"owner","payload":{"text":"do it","source":"human"}}),
        ),
        DEFAULT_LIMITS,
    );
    assert_eq!(view.feed.len(), 1);
    assert_eq!(view.feed[0].role, "owner");
    assert_eq!(view.feed[0].kind, "user_prompt");
    assert_eq!(view.feed[0].text, "do it");
}

#[test]
fn fold_sorts_by_seq_defensively() {
    let envelopes = vec![
        tool_call(2, "c2", "Bash", "second"),
        tool_call(0, "c0", "Bash", "zeroth"),
        tool_result(3, "c2", false),
        tool_call(1, "c1", "Bash", "first"),
    ];
    let view = fold_session_envelopes(&envelopes, DEFAULT_LIMITS);
    assert_eq!(view.last_seq, 3);
    assert_eq!(view.tools.len(), 3);
    assert_eq!(view.tools[0].display, "zeroth");
    assert_eq!(view.tools[1].display, "first");
    assert_eq!(view.tools[2].display, "second");
    // The result landed on the seq-2 call.
    assert!(view.tools[2].done);
}
