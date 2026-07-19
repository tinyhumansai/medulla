//! Tests for the read-only derivations: [`chat_transcript`],
//! [`last_assistant_message`], and [`describe_event`].

use serde_json::{json, Map};

use super::env;
use crate::ui::events::*;

#[test]
fn transcript_quotes_user_and_verbatim_assistant() {
    let events = vec![
        env(
            1,
            TuiEvent::User {
                body: "hi\n\nthere".into(),
            },
        ),
        env(
            2,
            TuiEvent::Assistant {
                body: "line1\nline2".into(),
            },
        ),
        env(
            3,
            TuiEvent::Error {
                source: "cycle".into(),
                message: "boom".into(),
            },
        ),
    ];
    let t = chat_transcript(&events);
    assert_eq!(t, "> hi\n>\n> there\n\nline1\nline2\n\n[error] cycle: boom");
}

#[test]
fn last_assistant_scans_from_end() {
    let events = vec![
        env(
            1,
            TuiEvent::Assistant {
                body: "first".into(),
            },
        ),
        env(
            2,
            TuiEvent::Assistant {
                body: "second".into(),
            },
        ),
    ];
    assert_eq!(last_assistant_message(&events).as_deref(), Some("second"));
}

#[test]
fn describe_event_snapshots() {
    let cases: Vec<(TuiEvent, &str)> = vec![
        (
            TuiEvent::CycleStart {
                cycle_id: "c1".into(),
            },
            "cycle started c1",
        ),
        (
            TuiEvent::CycleEnd {
                cycle_id: "c1".into(),
                pass_count: 2,
                duration_ms: 40,
            },
            "cycle finished · 2 passes · 40ms",
        ),
        (
            TuiEvent::InferenceStart {
                tier: "reasoning".into(),
                op: "step".into(),
                model: Some("gpt".into()),
            },
            "reasoning/step → gpt",
        ),
        (
            // model None → falls back to tier name.
            TuiEvent::InferenceStart {
                tier: "reasoning".into(),
                op: "step".into(),
                model: None,
            },
            "reasoning/step → reasoning",
        ),
        (
            TuiEvent::InferenceEnd {
                tier: "reasoning".into(),
                op: "step".into(),
                model: None,
                duration_ms: 7,
                usage: None,
                content: None,
                reasoning: None,
                tool_calls: None,
            },
            "reasoning/step ← 7ms",
        ),
        (
            TuiEvent::ToolCallStart {
                index: 3,
                name: "grep".into(),
            },
            "tool grep (call 3)",
        ),
        (
            TuiEvent::ToolCallDelta {
                index: 3,
                args_delta: "abcd".into(),
            },
            "tool args +4b (call 3)",
        ),
        (
            TuiEvent::AssistantDelta {
                delta: "hey".into(),
            },
            "assistant +3b",
        ),
        (
            TuiEvent::ReasoningDelta { delta: "yo".into() },
            "reasoning +2b",
        ),
        (
            TuiEvent::TaskStart {
                task_id: "t1".into(),
                instruction: "x".into(),
                depth: 2,
                agent_id: None,
            },
            "t1 started · depth 2",
        ),
        (
            TuiEvent::TaskEvent {
                task_id: "t1".into(),
                event_kind: "text".into(),
                content: "go".into(),
                harness: None,
            },
            "t1 · text: go",
        ),
        (
            TuiEvent::TaskAttention {
                task_id: "t1".into(),
                reason: "confirm".into(),
                content: "ok?".into(),
                question_id: None,
            },
            "t1 needs attention · confirm: ok?",
        ),
        (
            TuiEvent::AgentStatus {
                agent_id: "dev".into(),
                availability: "online".into(),
                detail: Some("idle".into()),
            },
            "agent dev · online · idle",
        ),
        (
            // no detail → no trailing segment.
            TuiEvent::AgentStatus {
                agent_id: "dev".into(),
                availability: "offline".into(),
                detail: None,
            },
            "agent dev · offline",
        ),
        (
            TuiEvent::PeerSession {
                agent_id: "m1".into(),
                session_id: "s1".into(),
                state: "working".into(),
                harness: Some("codex".into()),
            },
            "session s1 on m1 · working · codex",
        ),
        (
            TuiEvent::PeerSession {
                agent_id: "m1".into(),
                session_id: "s1".into(),
                state: "idle".into(),
                harness: None,
            },
            "session s1 on m1 · idle",
        ),
        (
            TuiEvent::SessionEvent {
                agent_id: "m1".into(),
                session_id: "s1".into(),
                event_kind: "stdout".into(),
                content: "log".into(),
            },
            "s1 · stdout: log",
        ),
        (
            TuiEvent::TaskComplete {
                digest: TaskDigest {
                    task_id: "t1".into(),
                    status: "done".into(),
                    digest: String::new(),
                    result_ref: None,
                    usage: None,
                    depth: 2,
                },
            },
            "t1 done",
        ),
        (
            // trace with op fallback (no tool).
            TuiEvent::Trace {
                entry: NodeTrace {
                    node: "orchestrate".into(),
                    ms: 5,
                    tool: None,
                    op: Some("delegate".into()),
                },
            },
            "orchestrate/delegate · 5ms",
        ),
        (
            // trace with neither tool nor op.
            TuiEvent::Trace {
                entry: NodeTrace {
                    node: "compress".into(),
                    ms: 8,
                    tool: None,
                    op: None,
                },
            },
            "compress · 8ms",
        ),
        (
            TuiEvent::Effect {
                effect: json!({"kind": "send_message"}),
            },
            "effect send_message",
        ),
        (
            // effect without a kind field → literal "effect".
            TuiEvent::Effect {
                effect: json!({"target": "x"}),
            },
            "effect effect",
        ),
        (TuiEvent::User { body: "hi".into() }, "you: hi"),
        (TuiEvent::Assistant { body: "ok".into() }, "assistant: ok"),
        (
            TuiEvent::Error {
                source: "cycle".into(),
                message: "boom".into(),
            },
            "cycle: boom",
        ),
        (
            TuiEvent::Unknown {
                kind: "weird".into(),
                data: Map::new(),
            },
            "event weird",
        ),
    ];
    for (ev, expected) in cases {
        assert_eq!(describe_event(&ev), expected);
    }
}

#[test]
fn trace_with_tool_prefers_tool_over_op() {
    let ev = TuiEvent::Trace {
        entry: NodeTrace {
            node: "orchestrate".into(),
            ms: 3,
            tool: Some("grep".into()),
            op: Some("ignored".into()),
        },
    };
    assert_eq!(describe_event(&ev), "orchestrate/grep · 3ms");
}

#[test]
fn transcript_skips_non_chat_events_and_joins_blocks() {
    let events = vec![
        env(
            1,
            TuiEvent::CycleStart {
                cycle_id: "c".into(),
            },
        ),
        env(2, TuiEvent::User { body: "q".into() }),
        env(
            3,
            TuiEvent::InferenceStart {
                tier: "r".into(),
                op: "o".into(),
                model: None,
            },
        ),
        env(4, TuiEvent::Assistant { body: "a".into() }),
    ];
    // Only the User and Assistant turns survive; the framing events are dropped.
    assert_eq!(chat_transcript(&events), "> q\n\na");
}

#[test]
fn transcript_and_last_empty_when_no_chat() {
    assert_eq!(chat_transcript(&[]), "");
    assert_eq!(last_assistant_message(&[]), None);
    // A stream with no assistant turn yields None for /copy last.
    let events = vec![env(1, TuiEvent::User { body: "q".into() })];
    assert_eq!(last_assistant_message(&events), None);
}
