//! Decode fixtures: event envelope/kind decoding, success/error envelope
//! unwrapping and error mapping, and `LoopEvent`/`RunResult` deserialization.

use crate::client::*;
use serde_json::{json, Value};

// ---------------------------------------------------------------------------
// Event envelope / kind decode fixtures
// ---------------------------------------------------------------------------

fn envelope(event: Value) -> EventEnvelope {
    let raw = json!({
        "seq": 5,
        "at": 1234,
        "sessionId": "s1",
        "cycleId": "c1",
        "event": event,
    });
    serde_json::from_value(raw).unwrap()
}

#[test]
fn decodes_user_and_assistant() {
    assert_eq!(
        envelope(json!({"kind": "user", "body": "hi"})).kind(),
        EventKind::User { body: "hi".into() }
    );
    assert_eq!(
        envelope(json!({"kind": "assistant", "body": "yo"})).kind(),
        EventKind::Assistant { body: "yo".into() }
    );
}

#[test]
fn decodes_cycle_bracket() {
    assert_eq!(
        envelope(json!({"kind": "cycle_start", "cycleId": "c1"})).kind(),
        EventKind::CycleStart {
            cycle_id: Some("c1".into())
        }
    );
    assert_eq!(
        envelope(json!({"kind": "cycle_end", "cycleId": "c1", "passCount": 3, "durationMs": 120}))
            .kind(),
        EventKind::CycleEnd {
            cycle_id: Some("c1".into()),
            pass_count: Some(3),
            duration_ms: Some(120),
            error: None,
        }
    );
    assert_eq!(
        envelope(json!({"kind": "cycle_end", "cycleId": "c1", "error": true})).kind(),
        EventKind::CycleEnd {
            cycle_id: Some("c1".into()),
            pass_count: None,
            duration_ms: None,
            error: Some(true),
        }
    );
}

#[test]
fn decodes_error_and_deltas() {
    assert_eq!(
        envelope(json!({"kind": "error", "source": "cycle", "message": "boom"})).kind(),
        EventKind::Error {
            source: "cycle".into(),
            message: "boom".into(),
        }
    );
    assert_eq!(
        envelope(json!({"kind": "assistant_delta", "delta": "to"})).kind(),
        EventKind::AssistantDelta { delta: "to".into() }
    );
    assert_eq!(
        envelope(json!({"kind": "reasoning_delta", "delta": "hm"})).kind(),
        EventKind::ReasoningDelta { delta: "hm".into() }
    );
    match envelope(json!({"kind": "tool_call_delta", "id": "t1"})).kind() {
        EventKind::ToolCallDelta { value } => assert_eq!(value["id"], json!("t1")),
        other => panic!("expected tool_call_delta, got {other:?}"),
    }
}

#[test]
fn unknown_kind_passthrough_preserves_raw() {
    let ev = envelope(json!({"kind": "future_thing", "payload": 9}));
    match ev.kind() {
        EventKind::Unknown(v) => {
            assert_eq!(v["kind"], json!("future_thing"));
            assert_eq!(v["payload"], json!(9));
        }
        other => panic!("expected unknown, got {other:?}"),
    }
    // Raw value stays accessible on the envelope.
    assert_eq!(ev.event["payload"], json!(9));
    assert_eq!(ev.seq, Some(5));
}

// ---------------------------------------------------------------------------
// SDK error mapping
// ---------------------------------------------------------------------------
//
// Envelope unwrapping itself now belongs to `tinyhumans-sdk`. What this crate
// still owns is the translation of the SDK's error taxonomy into [`ClientError`],
// which is what front ends branch on — so these cover that seam directly.

/// A failed non-2xx response, as the SDK reports it.
fn sdk_status(status: u16, body: Value) -> ClientError {
    tinyhumans_sdk::Error::Status { status, body }.into()
}

#[test]
fn maps_error_envelope_with_code() {
    let err = sdk_status(
        401,
        json!({ "success": false, "error": "token expired", "errorCode": "TOKEN_EXPIRED" }),
    );
    assert_eq!(err.error_code(), Some("TOKEN_EXPIRED"));
    assert!(err.is_token_expired());
    assert_eq!(err.status(), Some(401));
    match err {
        ClientError::Api { message, .. } => assert_eq!(message, "token expired"),
        other => panic!("expected api error, got {other:?}"),
    }
}

#[test]
fn maps_error_envelope_with_details() {
    let err = sdk_status(
        409,
        json!({
            "success": false,
            "error": "bad",
            "errorCode": "PROTOCOL_MISMATCH",
            "details": { "min": 1, "max": 2 },
        }),
    );
    match err {
        ClientError::Api { details, .. } => {
            let d = details.unwrap();
            assert_eq!(d["min"], json!(1));
            assert_eq!(d["max"], json!(2));
        }
        other => panic!("expected api error, got {other:?}"),
    }
}

#[test]
fn non_json_error_body_becomes_api_error() {
    // A non-JSON body reaches the SDK error as the raw text.
    let err = sdk_status(500, json!("internal error"));
    assert_eq!(err.status(), Some(500));
    assert_eq!(err.error_code(), None);
    match err {
        ClientError::Api { message, .. } => assert_eq!(message, "internal error"),
        other => panic!("expected api error, got {other:?}"),
    }
}

#[test]
fn error_body_without_a_message_falls_back_to_the_status() {
    let err = sdk_status(502, json!({ "unexpected": true }));
    match err {
        ClientError::Api { message, .. } => assert_eq!(message, "request failed with status 502"),
        other => panic!("expected api error, got {other:?}"),
    }
}

/// The backend does not always pair a failed operation with a non-2xx status.
/// The SDK reports that case separately, and it has to classify the same way.
#[test]
fn unsuccessful_envelope_on_a_2xx_maps_to_an_api_error() {
    let err: ClientError = tinyhumans_sdk::Error::Envelope {
        error: "token expired".into(),
        error_code: Some("TOKEN_EXPIRED".into()),
        details: Value::Null,
    }
    .into();
    assert!(err.is_token_expired());
    assert!(err.is_auth_error());
    // No HTTP status said so, so none is reported.
    assert_eq!(err.status(), None);
    match err {
        ClientError::Api {
            message, details, ..
        } => {
            assert_eq!(message, "token expired");
            assert!(details.is_none(), "a null details payload is dropped");
        }
        other => panic!("expected api error, got {other:?}"),
    }
}

/// A route the SDK intentionally does not expose fails before the network, so
/// it carries no status to misclassify as an auth failure.
#[test]
fn blocked_route_maps_to_a_statusless_api_error() {
    let err: ClientError =
        tinyhumans_sdk::Error::RouteNotExposed("GET".into(), "/coupons/admin".into()).into();
    assert_eq!(err.status(), None);
    assert_eq!(err.error_code(), None);
    assert!(!err.is_auth_error());
}

#[test]
fn auth_error_classification_covers_status_codes_and_non_api_errors() {
    let api_error = |status: Option<u16>, error_code: Option<&str>| ClientError::Api {
        status,
        message: "rejected".into(),
        error_code: error_code.map(str::to_owned),
        details: None,
    };

    assert!(api_error(Some(401), None).is_auth_error());
    assert!(api_error(Some(403), None).is_auth_error());
    assert!(api_error(Some(500), Some("TOKEN_EXPIRED")).is_auth_error());
    assert!(!api_error(Some(400), Some("INVALID_REQUEST")).is_auth_error());

    let decode = ClientError::Decode("bad response".into());
    assert_eq!(decode.error_code(), None);
    assert_eq!(decode.status(), None);
    assert!(!decode.is_token_expired());
    assert!(!decode.is_auth_error());
}

// ---------------------------------------------------------------------------
// LoopEvent / run result decode
// ---------------------------------------------------------------------------

#[test]
fn decodes_loop_events() {
    let tool_use: LoopEvent = serde_json::from_value(json!({
        "stop": "tool_use",
        "cycleId": "c1",
        "sessionId": "s1",
        "toolCalls": [{"id": "t1", "name": "search", "args": {"q": "x"}}],
    }))
    .unwrap();
    match tool_use {
        LoopEvent::ToolUse { tool_calls, .. } => {
            assert_eq!(tool_calls[0].name, "search");
            assert_eq!(tool_calls[0].args["q"], json!("x"));
        }
        other => panic!("expected tool_use, got {other:?}"),
    }

    let end: LoopEvent = serde_json::from_value(json!({
        "stop": "end",
        "cycleId": "c1",
        "sessionId": "s1",
        "reply": "done",
        "passCount": 2,
    }))
    .unwrap();
    match end {
        LoopEvent::End {
            reply, pass_count, ..
        } => {
            assert_eq!(reply, "done");
            assert_eq!(pass_count, Some(2));
        }
        other => panic!("expected end, got {other:?}"),
    }

    let pending: LoopEvent =
        serde_json::from_value(json!({"stop": "pending", "cycleId": "c1", "sessionId": "s1"}))
            .unwrap();
    assert!(matches!(pending, LoopEvent::Pending { .. }));
}

/// The SDK decides reply-vs-loop; this crate re-encodes either arm into its own
/// [`RunResult`]. Pin that both arms survive the round-trip with their fields.
#[test]
fn sdk_run_result_converts_into_client_run_result() {
    use tinyhumans_sdk::api::orchestration as sdk;

    let reply: RunResult = match sdk::RunResult::Reply(Box::new(sdk::RunReply {
        reply: "hello".into(),
        pass_count: Some(1),
        compressed_history: Vec::new(),
        escalations: Vec::new(),
        session_id: Some("s1".into()),
        cycle_id: Some("c1".into()),
    })) {
        sdk::RunResult::Reply(r) => RunResult::Reply(convert(*r).unwrap()),
        sdk::RunResult::Loop(e) => RunResult::Loop(convert(*e).unwrap()),
    };
    match reply {
        RunResult::Reply(reply) => {
            assert_eq!(reply.reply, "hello");
            assert_eq!(reply.pass_count, Some(1));
            assert_eq!(reply.cycle_id.as_deref(), Some("c1"));
        }
        other => panic!("expected reply, got {other:?}"),
    }

    let looped: RunResult = match sdk::RunResult::Loop(Box::new(sdk::LoopEvent::End {
        cycle_id: "c1".into(),
        session_id: "s1".into(),
        reply: "done".into(),
        pass_count: Some(2),
        compressed_history: Vec::new(),
        escalations: Vec::new(),
    })) {
        sdk::RunResult::Reply(r) => RunResult::Reply(convert(*r).unwrap()),
        sdk::RunResult::Loop(e) => RunResult::Loop(convert(*e).unwrap()),
    };
    match looped {
        RunResult::Loop(LoopEvent::End {
            reply, pass_count, ..
        }) => {
            assert_eq!(reply, "done");
            assert_eq!(pass_count, Some(2));
        }
        other => panic!("expected loop end, got {other:?}"),
    }
}

#[test]
fn run_options_serialize_camel_case() {
    let opts = RunOptions {
        session_id: Some("s1".into()),
        tools: None,
        options: Some(RunOrchestrationOptions {
            config: Some(RunConfig {
                max_passes: Some(4),
                ..Default::default()
            }),
            ..Default::default()
        }),
    };
    let v = serde_json::to_value(&opts).unwrap();
    assert_eq!(v["sessionId"], json!("s1"));
    assert_eq!(v["options"]["config"]["maxPasses"], json!(4));
    // Unset fields are omitted.
    assert!(v.get("tools").is_none());
    assert!(v["options"].get("workspaceProfiles").is_none());
}

#[test]
fn workspace_profiles_serialize_camel_case() {
    let opts = RunOptions {
        options: Some(RunOrchestrationOptions {
            workspace_profiles: Some(vec![WorkspaceProfileInput {
                workspace: "/repo/pay".into(),
                medulla_md: "Payments service.".into(),
            }]),
            ..Default::default()
        }),
        ..Default::default()
    };
    let v = serde_json::to_value(&opts).unwrap();
    let profiles = &v["options"]["workspaceProfiles"];
    assert_eq!(profiles[0]["workspace"], json!("/repo/pay"));
    // Sent verbatim as `medullaMd` — the backend/SDK owns parsing.
    assert_eq!(profiles[0]["medullaMd"], json!("Payments service."));
}
