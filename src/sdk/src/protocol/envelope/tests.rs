//! Conformance tests for the vendored harness envelope wire format.
//!
//! Ported verbatim from the tiny.place SDK the types used to ship in, so the
//! serde representation this repo now owns is pinned to what deployed wrappers
//! already emit. The snake_case round-trip assertions are the load-bearing ones.

use super::v1::*;
use super::v2::*;
use super::AnySessionEnvelope;

const SAMPLE: &str = r#"{
    "envelope_version": "tinyplace.harness.session.v1",
    "version": 1,
    "bucket": { "unit": "hour", "start": "s", "end": "e" },
    "scope": { "type": "session", "key": "k", "cwd": "/repo",
               "wrapper_session_id": "w1", "harness_session_id": "h1" },
    "harness": { "provider": "claude", "command": "claude", "argv": ["-p"] },
    "message": { "id": "m1", "line": 3, "role": "agent", "text": "hi",
                 "timestamp": "2026-07-02T00:00:00Z" },
    "source": { "path": "p", "record_type": "assistant" }
}"#;

#[test]
fn parses_and_round_trips_v1() {
    let env = SessionEnvelopeV1::parse(SAMPLE).expect("valid v1 envelope");
    assert_eq!(env.scope.harness_session_id, "h1");
    assert_eq!(env.scope.scope_type, "session");
    assert_eq!(env.message.role, "agent");
    assert_eq!(env.harness.provider, "claude");
    assert_eq!(env.message.line, 3);

    // snake_case must round-trip — regression guard against a camelCase rename.
    let json = serde_json::to_string(&env).unwrap();
    assert!(json.contains("\"harness_session_id\""));
    assert!(json.contains("\"envelope_version\""));
    assert!(json.contains("\"record_type\""));
}

#[test]
fn rejects_unknown_version_and_plain_dm() {
    assert!(SessionEnvelopeV1::parse(
        r#"{"envelope_version":"other","scope":{"harness_session_id":"h"}}"#
    )
    .is_none());
    assert!(SessionEnvelopeV1::parse("just a normal message").is_none());
    assert!(SessionEnvelopeV1::parse(
        r#"{"envelope_version":"tinyplace.harness.session.v1","scope":{"harness_session_id":""}}"#
    )
    .is_none());
}

#[test]
fn v1_session_key_is_the_shared_wrapper_id_then_harness_fallback() {
    // The single per-pair id lives in `wrapper_session_id`.
    assert_eq!(
        SessionEnvelopeV1::parse(SAMPLE).unwrap().session_key(),
        "w1"
    );
    // Legacy envelope with no per-pair id: fall back to the harness id.
    let env = SessionEnvelopeV1::parse(
        r#"{
            "envelope_version": "tinyplace.harness.session.v1",
            "scope": { "harness_session_id": "h-only" }
        }"#,
    )
    .expect("valid v1");
    assert_eq!(env.session_key(), "h-only");
}

#[test]
fn v1_outgoing_builds_a_parseable_envelope() {
    let env = SessionEnvelopeV1::outgoing("h9", "reply body", "m9", "2026-07-04T00:00:00Z");
    let wire = serde_json::to_string(&env).expect("encode");
    let parsed = SessionEnvelopeV1::parse(&wire).expect("valid v1");
    assert_eq!(parsed.scope.harness_session_id, "h9");
    assert_eq!(parsed.scope.wrapper_session_id, "h9");
    assert_eq!(parsed.message.text, "reply body");
    assert_eq!(parsed.message.role, "owner");
}

// ── v2 envelope ─────────────────────────────────────────────────────────

/// Build a v2 envelope wire string with the given `kind` + `payload` JSON.
fn v2_wire(kind: &str, payload: &str) -> String {
    format!(
        r#"{{
            "envelope_version": "tinyplace.harness.session.v2",
            "version": 2,
            "bucket": {{ "unit": "minute", "start": "s", "end": "e" }},
            "scope": {{ "type": "folder", "key": "repo", "cwd": "/w",
                       "wrapper_session_id": "w2", "harness_session_id": "h2" }},
            "harness": {{ "provider": "claude", "command": "claude", "argv": [] }},
            "event": {{ "id": "e1", "seq": 4, "ts": "2026-07-05T00:00:00Z",
                       "turn_id": "t1", "model": "opus", "role": "agent",
                       "kind": "{kind}", "payload": {payload} }},
            "source": {{ "path": "p", "record_type": "assistant" }}
        }}"#
    )
}

#[test]
fn parses_valid_v2_envelope_and_common_event_fields() {
    let wire = v2_wire("agent_message", r#"{ "text": "hi there" }"#);
    let env = SessionEnvelopeV2::parse(&wire).expect("valid v2");
    assert_eq!(env.envelope_version, SESSION_ENVELOPE_VERSION_V2);
    assert_eq!(env.version, 2);
    assert_eq!(env.scope.wrapper_session_id, "w2");
    assert_eq!(env.harness.provider, "claude");
    assert_eq!(env.event.id, "e1");
    assert_eq!(env.event.seq, 4);
    assert_eq!(env.event.turn_id.as_deref(), Some("t1"));
    assert_eq!(env.event.model.as_deref(), Some("opus"));
    assert_eq!(env.event.role, "agent");
    assert_eq!(env.session_key(), "w2");

    // snake_case must round-trip — regression guard against a camelCase rename.
    let json = serde_json::to_string(&env).unwrap();
    assert!(json.contains("\"harness_session_id\""));
    assert!(json.contains("\"envelope_version\""));
    assert!(json.contains("\"record_type\""));
}

#[test]
fn v2_decodes_every_event_kind() {
    use HarnessEventKind::*;

    let up = SessionEnvelopeV2::parse(&v2_wire(
        "user_prompt",
        r#"{ "text": "do it", "source": "human" }"#,
    ))
    .unwrap();
    assert_eq!(
        up.event.decoded(),
        UserPrompt(UserPromptPayload {
            text: "do it".into(),
            source: "human".into()
        })
    );

    let am = SessionEnvelopeV2::parse(&v2_wire("agent_message", r#"{ "text": "ok" }"#)).unwrap();
    assert_eq!(
        am.event.decoded(),
        AgentMessage(TextPayload { text: "ok".into() })
    );

    let th = SessionEnvelopeV2::parse(&v2_wire("agent_thinking", r#"{ "text": "hmm" }"#)).unwrap();
    assert_eq!(
        th.event.decoded(),
        AgentThinking(TextPayload { text: "hmm".into() })
    );

    let tc = SessionEnvelopeV2::parse(&v2_wire(
        "tool_call",
        r#"{ "call_id": "c1", "tool_name": "Bash", "tool_kind": "shell",
             "display": "ls -la", "input": { "cmd": "ls" } }"#,
    ))
    .unwrap();
    match tc.event.decoded() {
        ToolCall(p) => {
            assert_eq!(p.call_id, "c1");
            assert_eq!(p.tool_name, "Bash");
            assert_eq!(p.tool_kind, "shell");
            assert_eq!(p.display, "ls -la");
            assert_eq!(p.input["cmd"], "ls");
        }
        other => panic!("expected tool_call, got {other:?}"),
    }

    let tr = SessionEnvelopeV2::parse(&v2_wire(
        "tool_result",
        r#"{ "call_id": "c1", "ok": true, "exit_code": 0, "is_error": false,
             "output": "done", "output_bytes": 4 }"#,
    ))
    .unwrap();
    assert_eq!(
        tr.event.decoded(),
        ToolResult(ToolResultPayload {
            call_id: "c1".into(),
            ok: true,
            exit_code: Some(0),
            is_error: false,
            output: "done".into(),
            output_bytes: 4,
        })
    );

    let ar = SessionEnvelopeV2::parse(&v2_wire(
        "approval_request",
        r#"{ "call_id": "c9", "tool_name": "rm", "display": "rm -rf x", "reason": "destructive" }"#,
    ))
    .unwrap();
    assert_eq!(
        ar.event.decoded(),
        ApprovalRequest(ApprovalRequestPayload {
            call_id: Some("c9".into()),
            tool_name: "rm".into(),
            display: "rm -rf x".into(),
            reason: Some("destructive".into()),
        })
    );

    let st = SessionEnvelopeV2::parse(&v2_wire(
        "status",
        r#"{ "state": "running_tool", "detail": "compiling", "active_call_id": "c1" }"#,
    ))
    .unwrap();
    assert_eq!(
        st.event.decoded(),
        Status(StatusPayload {
            state: "running_tool".into(),
            detail: "compiling".into(),
            active_call_id: Some("c1".into()),
        })
    );

    let lc =
        SessionEnvelopeV2::parse(&v2_wire("lifecycle", r#"{ "phase": "session_end" }"#)).unwrap();
    assert_eq!(
        lc.event.decoded(),
        Lifecycle(LifecyclePayload {
            phase: "session_end".into()
        })
    );

    let er = SessionEnvelopeV2::parse(&v2_wire("error", r#"{ "message": "boom", "fatal": true }"#))
        .unwrap();
    assert_eq!(
        er.event.decoded(),
        Error(ErrorPayload {
            message: "boom".into(),
            fatal: true
        })
    );

    let uk = SessionEnvelopeV2::parse(&v2_wire("unknown", r#"{ "raw": { "x": 1 } }"#)).unwrap();
    match uk.event.decoded() {
        Unknown(p) => assert_eq!(p.raw["x"], 1),
        other => panic!("expected unknown, got {other:?}"),
    }
}

#[test]
fn v2_unrecognised_kind_folds_to_unknown_not_a_parse_error() {
    // A future kind the receiver doesn't model must not fail the envelope
    // parse (which would silently route the DM elsewhere); it folds to
    // Unknown carrying the raw payload.
    let env = SessionEnvelopeV2::parse(&v2_wire("quantum_teleport", r#"{ "flux": 42 }"#))
        .expect("still a valid v2 envelope");
    match env.event.decoded() {
        HarnessEventKind::Unknown(p) => assert_eq!(p.raw["flux"], 42),
        other => panic!("expected unknown fold, got {other:?}"),
    }
}

#[test]
fn v1_and_v2_bodies_do_not_cross_parse() {
    // A v1 envelope must NOT parse as v2 (discriminated on envelope_version).
    assert!(SessionEnvelopeV2::parse(SAMPLE).is_none());
    assert!(SessionEnvelopeV2::parse("a plain message").is_none());
    // Right shape, wrong version string.
    assert!(SessionEnvelopeV2::parse(
        r#"{"envelope_version":"tinyplace.harness.session.v3","scope":{"harness_session_id":"h"}}"#
    )
    .is_none());
    // Correct version but empty harness id → invalid.
    assert!(SessionEnvelopeV2::parse(
        r#"{"envelope_version":"tinyplace.harness.session.v2","scope":{"harness_session_id":""}}"#
    )
    .is_none());
    // Conversely a v2 body is not a v1 envelope.
    let v2 = v2_wire("agent_message", r#"{ "text": "x" }"#);
    assert!(SessionEnvelopeV1::parse(&v2).is_none());
}

#[test]
fn v2_session_key_falls_back_to_harness_id() {
    let env = SessionEnvelopeV2::parse(
        r#"{
            "envelope_version": "tinyplace.harness.session.v2",
            "scope": { "harness_session_id": "h-only" },
            "event": { "kind": "agent_message", "payload": { "text": "x" } }
        }"#,
    )
    .expect("valid v2");
    assert_eq!(env.session_key(), "h-only");
}

#[test]
fn any_session_envelope_parses_both_versions() {
    match AnySessionEnvelope::parse(SAMPLE) {
        Some(AnySessionEnvelope::V1(env)) => assert_eq!(env.session_key(), "w1"),
        other => panic!("expected V1, got {other:?}"),
    }

    let v2 = v2_wire("agent_message", r#"{ "text": "hi" }"#);
    match AnySessionEnvelope::parse(&v2) {
        Some(AnySessionEnvelope::V2(env)) => {
            assert_eq!(env.session_key(), "w2");
            assert_eq!(
                env.event.decoded(),
                HarnessEventKind::AgentMessage(TextPayload { text: "hi".into() })
            );
        }
        other => panic!("expected V2, got {other:?}"),
    }

    assert!(AnySessionEnvelope::parse("a plain DM").is_none());
}
