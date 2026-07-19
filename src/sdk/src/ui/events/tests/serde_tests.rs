//! Tests for [`TuiEvent`] JSON serialization: full round-trips across every
//! kind, and the deserialize tolerance rules (unknown kinds, missing fields,
//! empty-string normalization, and error cases).

use serde_json::json;

use crate::ui::events::*;

#[test]
fn unknown_kind_round_trips() {
    let json = r#"{"kind":"weird_kind","payload":42}"#;
    let ev: TuiEvent = serde_json::from_str(json).unwrap();
    match &ev {
        TuiEvent::Unknown { kind, data } => {
            assert_eq!(kind, "weird_kind");
            assert_eq!(data.get("payload").unwrap(), &json!(42));
        }
        _ => panic!("expected unknown"),
    }
    let back = serde_json::to_value(&ev).unwrap();
    assert_eq!(back.get("kind").unwrap(), &json!("weird_kind"));
    assert_eq!(back.get("payload").unwrap(), &json!(42));
}

#[test]
fn known_event_round_trips() {
    let ev = TuiEvent::InferenceEnd {
        tier: "reasoning".into(),
        op: "execute_step".into(),
        model: Some("gpt".into()),
        duration_ms: 120,
        usage: Some(Usage {
            input_tokens: 10,
            output_tokens: 5,
        }),
        content: None,
        reasoning: None,
        tool_calls: None,
    };
    let s = serde_json::to_string(&ev).unwrap();
    let back: TuiEvent = serde_json::from_str(&s).unwrap();
    assert_eq!(ev, back);
}

/// One representative JSON per kind, exercising every deserialize arm.
fn one_of_each() -> Vec<(&'static str, TuiEvent)> {
    vec![
        (
            "inference_start",
            TuiEvent::InferenceStart {
                tier: "orchestrator".into(),
                op: "orchestrate".into(),
                model: Some("m".into()),
            },
        ),
        (
            "inference_end",
            TuiEvent::InferenceEnd {
                tier: "reasoning".into(),
                op: "step".into(),
                model: None,
                duration_ms: 5,
                usage: None,
                content: Some("c".into()),
                reasoning: Some("r".into()),
                tool_calls: Some(vec![ToolCall {
                    name: "grep".into(),
                    args: json!({"q": 1}),
                }]),
            },
        ),
        (
            "tool_call_start",
            TuiEvent::ToolCallStart {
                index: 2,
                name: "read".into(),
            },
        ),
        (
            "tool_call_delta",
            TuiEvent::ToolCallDelta {
                index: 2,
                args_delta: "{\"a\":".into(),
            },
        ),
        (
            "assistant_delta",
            TuiEvent::AssistantDelta { delta: "x".into() },
        ),
        (
            "reasoning_delta",
            TuiEvent::ReasoningDelta { delta: "y".into() },
        ),
        (
            "task_start",
            TuiEvent::TaskStart {
                task_id: "t1".into(),
                instruction: "do".into(),
                depth: 2,
                agent_id: Some("dev".into()),
            },
        ),
        (
            "task_event",
            TuiEvent::TaskEvent {
                task_id: "t1".into(),
                event_kind: "text".into(),
                content: "hi".into(),
                harness: Some("codex".into()),
            },
        ),
        (
            "task_attention",
            TuiEvent::TaskAttention {
                task_id: "t1".into(),
                reason: "confirm".into(),
                content: "proceed?".into(),
                question_id: Some("q1".into()),
            },
        ),
        (
            "task_complete",
            TuiEvent::TaskComplete {
                digest: TaskDigest {
                    task_id: "t1".into(),
                    status: "done".into(),
                    digest: "d".into(),
                    result_ref: Some(json!({"ref": 1})),
                    usage: Some(Usage {
                        input_tokens: 1,
                        output_tokens: 2,
                    }),
                    depth: 2,
                },
            },
        ),
        (
            "trace",
            TuiEvent::Trace {
                entry: NodeTrace {
                    node: "orchestrate".into(),
                    ms: 12,
                    tool: Some("grep".into()),
                    op: None,
                },
            },
        ),
        (
            "error",
            TuiEvent::Error {
                source: "cycle".into(),
                message: "boom".into(),
            },
        ),
        (
            "cycle_start",
            TuiEvent::CycleStart {
                cycle_id: "c1".into(),
            },
        ),
        (
            "cycle_end",
            TuiEvent::CycleEnd {
                cycle_id: "c1".into(),
                pass_count: 3,
                duration_ms: 99,
            },
        ),
        (
            "agent_status",
            TuiEvent::AgentStatus {
                agent_id: "dev".into(),
                availability: "online".into(),
                detail: Some("idle".into()),
            },
        ),
        (
            "session_event",
            TuiEvent::SessionEvent {
                agent_id: "m1".into(),
                session_id: "s1".into(),
                event_kind: "stdout".into(),
                content: "log".into(),
            },
        ),
        (
            "peer_session",
            TuiEvent::PeerSession {
                agent_id: "m1".into(),
                session_id: "s1".into(),
                state: "working".into(),
                harness: Some("codex".into()),
            },
        ),
        ("user", TuiEvent::User { body: "hey".into() }),
        ("assistant", TuiEvent::Assistant { body: "yo".into() }),
        (
            "effect",
            TuiEvent::Effect {
                effect: json!({"kind": "send"}),
            },
        ),
    ]
}

#[test]
fn every_kind_round_trips_and_reports_kind() {
    for (kind, ev) in one_of_each() {
        assert_eq!(ev.kind(), kind, "kind() mismatch for {kind}");
        let s = serde_json::to_string(&ev).unwrap();
        let back: TuiEvent = serde_json::from_str(&s).unwrap();
        assert_eq!(ev, back, "round-trip mismatch for {kind}");
        // describe_event never panics and is non-empty.
        assert!(!describe_event(&ev).is_empty(), "empty describe for {kind}");
    }
}

#[test]
fn empty_object_is_unknown_with_empty_kind() {
    let ev: TuiEvent = serde_json::from_str("{}").unwrap();
    assert!(matches!(&ev, TuiEvent::Unknown { kind, .. } if kind.is_empty()));
    assert_eq!(ev.kind(), "");
}

#[test]
fn non_object_json_is_a_deserialize_error() {
    assert!(serde_json::from_str::<TuiEvent>("[1,2,3]").is_err());
    assert!(serde_json::from_str::<TuiEvent>("42").is_err());
}

#[test]
fn task_complete_without_digest_errors() {
    assert!(serde_json::from_str::<TuiEvent>(r#"{"kind":"task_complete"}"#).is_err());
}

#[test]
fn trace_without_entry_errors() {
    assert!(serde_json::from_str::<TuiEvent>(r#"{"kind":"trace"}"#).is_err());
}

#[test]
fn opt_str_filters_empty_to_none() {
    // An empty `model` string decodes to `None`, not `Some("")`.
    let ev: TuiEvent =
        serde_json::from_str(r#"{"kind":"inference_start","tier":"r","op":"o","model":""}"#)
            .unwrap();
    assert!(matches!(ev, TuiEvent::InferenceStart { model: None, .. }));
}

#[test]
fn serialize_drops_null_fields() {
    // A model-less inference_start must not carry a `"model":null` key.
    let ev = TuiEvent::InferenceStart {
        tier: "r".into(),
        op: "o".into(),
        model: None,
    };
    let v = serde_json::to_value(&ev).unwrap();
    assert!(v.get("model").is_none(), "null model should be dropped");
    assert_eq!(v.get("kind").unwrap(), &json!("inference_start"));
}

#[test]
fn effect_decode_defaults_to_null_when_missing() {
    let ev: TuiEvent = serde_json::from_str(r#"{"kind":"effect"}"#).unwrap();
    assert!(matches!(ev, TuiEvent::Effect { effect } if effect.is_null()));
}

#[test]
fn envelope_round_trips() {
    let e = EventEnvelope {
        seq: 7,
        at: 123,
        event: TuiEvent::User { body: "hi".into() },
    };
    let s = serde_json::to_string(&e).unwrap();
    let back: EventEnvelope = serde_json::from_str(&s).unwrap();
    assert_eq!(e, back);
}

#[test]
fn tool_call_defaults_args_to_null() {
    let tc: ToolCall = serde_json::from_str(r#"{"name":"grep"}"#).unwrap();
    assert_eq!(tc.name, "grep");
    assert!(tc.args.is_null());
}
