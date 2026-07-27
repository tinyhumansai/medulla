//! Unit tests for the `medulla-tinyplace/1` frame codec: encode/decode
//! round-trips, optional-field handling, and tolerant capabilities parsing.

use crate::tinyplace::{
    decode_task_frame, encode_task_frame, parse_agent_capabilities, BudgetSource, BudgetWindow,
    EncodeFrameInput, HarnessBudget, HarnessProvider, HarnessReadiness, TaskFrameKind,
    TINYPLACE_PROTO,
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
        workflow: None,
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
        workflow: Some("nightly-sweep".to_string()),
    });
    let value: serde_json::Value = serde_json::from_str(&body).unwrap();
    assert_eq!(value["kind"], "capabilities_result");
    assert_eq!(value["correlationId"], "corr-9");
    assert_eq!(value["harness"], "codex");
    assert_eq!(value["provider"], "claude");
    assert_eq!(value["model"], "anthropic/claude-opus-4.8");
    assert_eq!(value["workflow"], "nightly-sweep");
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
        (TaskFrameKind::SystemInfo, "system_info"),
        (TaskFrameKind::SystemInfoResult, "system_info_result"),
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
            workflow: None,
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
        workflow: None,
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
fn harness_budget_uses_camelcase_keys_and_snake_enum_values() {
    let budget = HarnessBudget {
        provider: HarnessProvider::Claude,
        seat: Some("seat-9".to_string()),
        window: BudgetWindow::FiveHour,
        limit_tokens: Some(1_000),
        used_tokens: Some(250),
        remaining_tokens: Some(750),
        cooldown_until: Some(1_800_000_000),
        source: BudgetSource::ProviderReported,
    };
    let value = serde_json::to_value(&budget).unwrap();
    assert_eq!(value["provider"], "claude");
    assert_eq!(value["seat"], "seat-9");
    // Enum values are snake_case; numeric keys are camelCase.
    assert_eq!(value["window"], "five_hour");
    assert_eq!(value["limitTokens"], 1_000);
    assert_eq!(value["usedTokens"], 250);
    assert_eq!(value["remainingTokens"], 750);
    assert_eq!(value["cooldownUntil"], 1_800_000_000_i64);
    assert_eq!(value["source"], "provider_reported");
    // Round-trips cleanly.
    let back: HarnessBudget = serde_json::from_value(value).unwrap();
    assert_eq!(back, budget);
}

#[test]
fn harness_budget_omits_absent_optionals() {
    let budget = HarnessBudget {
        provider: HarnessProvider::Codex,
        seat: None,
        window: BudgetWindow::Unknown,
        limit_tokens: None,
        used_tokens: None,
        remaining_tokens: None,
        cooldown_until: None,
        source: BudgetSource::Estimate,
    };
    let value = serde_json::to_value(&budget).unwrap();
    assert_eq!(value["window"], "unknown");
    assert_eq!(value["source"], "estimate");
    // All optional numeric/seat/cooldown keys are dropped when absent.
    for key in [
        "seat",
        "limitTokens",
        "usedTokens",
        "remainingTokens",
        "cooldownUntil",
    ] {
        assert!(value.get(key).is_none(), "{key} should be omitted");
    }
    let back: HarnessBudget = serde_json::from_value(value).unwrap();
    assert_eq!(back, budget);
}

#[test]
fn harness_readiness_omits_reason_when_ready() {
    let ready = HarnessReadiness {
        provider: HarnessProvider::Opencode,
        ready: true,
        reason: None,
    };
    let value = serde_json::to_value(&ready).unwrap();
    assert_eq!(value["provider"], "opencode");
    assert_eq!(value["ready"], true);
    assert!(value.get("reason").is_none());

    let not_ready = HarnessReadiness {
        provider: HarnessProvider::Opencode,
        ready: false,
        reason: Some("not authenticated".to_string()),
    };
    let value = serde_json::to_value(&not_ready).unwrap();
    assert_eq!(value["ready"], false);
    assert_eq!(value["reason"], "not authenticated");
}

#[test]
fn budget_window_defaults_to_unknown() {
    assert_eq!(BudgetWindow::default(), BudgetWindow::Unknown);
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

#[test]
fn old_capabilities_without_budgets_parse_to_empty() {
    // A peer that predates the budget surface omits the keys entirely.
    let text = json!({
        "cwd": "/repo",
        "providers": ["claude"],
        "tools": ["Bash"],
    })
    .to_string();
    let caps = parse_agent_capabilities(&text).unwrap();
    assert!(caps.budgets.is_empty(), "absent budgets → empty");
    assert!(caps.readiness.is_empty(), "absent readiness → empty");
}

#[test]
fn empty_budgets_and_readiness_are_omitted_on_the_wire() {
    // A new peer with nothing to advertise serializes a frame an old peer parses.
    let caps = crate::tinyplace::AgentCapabilities {
        cwd: Some("/repo".to_string()),
        providers: vec![HarnessProvider::Claude],
        ..Default::default()
    };
    let value = serde_json::to_value(&caps).unwrap();
    assert!(value.get("budgets").is_none());
    assert!(value.get("readiness").is_none());
}

#[test]
fn new_capabilities_round_trip_budgets_and_readiness() {
    let caps = crate::tinyplace::AgentCapabilities {
        cwd: Some("/repo".to_string()),
        providers: vec![HarnessProvider::Claude, HarnessProvider::Codex],
        budgets: vec![HarnessBudget {
            provider: HarnessProvider::Claude,
            seat: None,
            window: BudgetWindow::Unknown,
            limit_tokens: None,
            used_tokens: None,
            remaining_tokens: None,
            cooldown_until: None,
            source: BudgetSource::Estimate,
        }],
        readiness: vec![
            HarnessReadiness {
                provider: HarnessProvider::Claude,
                ready: true,
                reason: None,
            },
            HarnessReadiness {
                provider: HarnessProvider::Codex,
                ready: false,
                reason: Some("not authenticated".to_string()),
            },
        ],
        ..Default::default()
    };
    let text = serde_json::to_string(&caps).unwrap();
    // Parsed back through the tolerant frame parser, the budget surface survives.
    let back = parse_agent_capabilities(&text).unwrap();
    assert_eq!(back.budgets, caps.budgets);
    assert_eq!(back.readiness, caps.readiness);
    assert_eq!(back.providers, caps.providers);
}

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

    let body = crate::tinyplace::encode_task_frame_with_work(
        EncodeFrameInput {
            kind: TaskFrameKind::Status,
            task_id: "cycle-1".to_string(),
            text: "write the fold · todo 1/2".to_string(),
            ts: "2026-07-18T00:00:00.000Z".to_string(),
            correlation_id: None,
            harness: Some(HarnessProvider::Claude),
            provider: None,
            model: None,
            workflow: None,
        },
        None,
        Some(snapshot.clone()),
    );
    let decoded = decode_task_frame(&body).expect("the frame decodes");
    assert_eq!(decoded.work.as_deref(), Some(&snapshot));
}

#[test]
fn an_empty_work_snapshot_is_left_off_the_wire() {
    let body = crate::tinyplace::encode_task_frame_with_work(
        EncodeFrameInput {
            kind: TaskFrameKind::Status,
            task_id: "cycle-1".to_string(),
            text: "thinking".to_string(),
            ts: "2026-07-18T00:00:00.000Z".to_string(),
            correlation_id: None,
            harness: None,
            provider: None,
            model: None,
            workflow: None,
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
    // A peer on a shape we cannot read must still deliver its reply: the task
    // result is the payload that matters.
    let body = json!({
        "proto": TINYPLACE_PROTO,
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
