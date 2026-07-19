//! Unit tests for the `medulla-tinyplace/1` frame codec: encode/decode
//! round-trips, optional-field handling, and tolerant capabilities parsing.

use crate::tinyplace_support::{
    decode_task_frame, encode_task_frame, parse_agent_capabilities, EncodeFrameInput,
    HarnessProvider, TaskFrameKind, TINYPLACE_PROTO,
};
use serde_json::json;

#[test]
fn encodes_a_minimal_frame() {
    let body = encode_task_frame(EncodeFrameInput {
        kind: TaskFrameKind::Task,
        task_id: "cycle-1".to_string(),
        text: "do the thing".to_string(),
        ts: "2026-07-18T00:00:00.000Z".to_string(),
        correlation_id: None,
        harness: None,
        provider: None,
        model: None,
    });
    let value: serde_json::Value = serde_json::from_str(&body).unwrap();
    assert_eq!(value["proto"], TINYPLACE_PROTO);
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

#[test]
fn encodes_optional_fields_when_present() {
    let body = encode_task_frame(EncodeFrameInput {
        kind: TaskFrameKind::CapabilitiesResult,
        task_id: "t".to_string(),
        text: "{}".to_string(),
        ts: "2026-07-18T00:00:00.000Z".to_string(),
        correlation_id: Some("corr-9".to_string()),
        harness: Some(HarnessProvider::Codex),
        provider: Some(HarnessProvider::Claude),
        model: Some("anthropic/claude-opus-4.8".to_string()),
    });
    let value: serde_json::Value = serde_json::from_str(&body).unwrap();
    assert_eq!(value["kind"], "capabilities_result");
    assert_eq!(value["correlationId"], "corr-9");
    assert_eq!(value["harness"], "codex");
    assert_eq!(value["provider"], "claude");
    assert_eq!(value["model"], "anthropic/claude-opus-4.8");
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
    ] {
        let body = encode_task_frame(EncodeFrameInput {
            kind,
            task_id: "t".to_string(),
            text: "x".to_string(),
            ts: "ts".to_string(),
            correlation_id: None,
            harness: None,
            provider: None,
            model: None,
        });
        let decoded = decode_task_frame(&body).expect("valid frame decodes");
        assert_eq!(decoded.kind, kind);
        assert_eq!(decoded.kind.as_str(), wire);
    }
}

#[test]
fn decodes_a_full_frame() {
    let body = json!({
        "proto": TINYPLACE_PROTO,
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
        kind: TaskFrameKind::Task,
        task_id: "t".to_string(),
        text: "x".to_string(),
        ts: "ts".to_string(),
        correlation_id: None,
        harness: None,
        provider: None,
        model: Some("openrouter/some-model".to_string()),
    });
    let decoded = decode_task_frame(&body).unwrap();
    assert_eq!(decoded.model.as_deref(), Some("openrouter/some-model"));
}

#[test]
fn decode_treats_absent_or_blank_model_as_none() {
    // Absent entirely.
    let absent = json!({
        "proto": TINYPLACE_PROTO, "kind": "task", "taskId": "t", "text": "x", "ts": "ts",
    })
    .to_string();
    assert_eq!(decode_task_frame(&absent).unwrap().model, None);
    // Present but blank — treated as no hint so the daemon keeps its default.
    let blank = json!({
        "proto": TINYPLACE_PROTO, "kind": "task", "taskId": "t", "text": "x", "ts": "ts",
        "model": "   ",
    })
    .to_string();
    assert_eq!(decode_task_frame(&blank).unwrap().model, None);
}

#[test]
fn decode_tolerates_missing_ts() {
    let body = json!({
        "proto": TINYPLACE_PROTO,
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
        "proto": TINYPLACE_PROTO,
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
        &json!({"proto": TINYPLACE_PROTO, "kind": "nope", "taskId": "t", "text": "x"}).to_string()
    )
    .is_none());
    // Missing required text.
    assert!(decode_task_frame(
        &json!({"proto": TINYPLACE_PROTO, "kind": "task", "taskId": "t"}).to_string()
    )
    .is_none());
}

#[test]
fn parses_agent_capabilities() {
    let text = json!({
        "cwd": "/repo",
        "accessibleDirs": ["/repo", "/tmp", "", "  "],
        "project": "medulla",
        "branch": "main",
        "providers": ["claude", "codex", "gemini"],
        "tools": ["Bash", "Read"],
        "mcpServers": ["langfuse"],
        "summary": "coding agent",
    })
    .to_string();
    let caps = parse_agent_capabilities(&text).unwrap();
    assert_eq!(caps.cwd.as_deref(), Some("/repo"));
    // Blank entries dropped, real ones trimmed/kept.
    assert_eq!(caps.accessible_dirs, vec!["/repo", "/tmp"]);
    assert_eq!(caps.project.as_deref(), Some("medulla"));
    // Unknown providers filtered out.
    assert_eq!(
        caps.providers,
        vec![HarnessProvider::Claude, HarnessProvider::Codex]
    );
    assert_eq!(caps.tools, vec!["Bash", "Read"]);
    assert_eq!(caps.mcp_servers, vec!["langfuse"]);
    assert_eq!(caps.summary.as_deref(), Some("coding agent"));
}

#[test]
fn parse_agent_capabilities_defaults_missing_arrays() {
    let caps = parse_agent_capabilities(r#"{"cwd":"/x"}"#).unwrap();
    assert!(caps.accessible_dirs.is_empty());
    assert!(caps.providers.is_empty());
    assert!(caps.tools.is_empty());
    assert!(caps.mcp_servers.is_empty());
    assert!(parse_agent_capabilities("[]").is_none());
    assert!(parse_agent_capabilities("nope").is_none());
}
