//! Unit tests for the `medulla-task/1` frame codec: encode/decode
//! round-trips, optional-field handling, and tolerant capabilities parsing.

use crate::protocol::{
    decode_task_frame, encode_task_frame, parse_agent_capabilities, BudgetSource, BudgetWindow,
    EncodeFrameInput, HarnessBudget, HarnessProvider, HarnessReadiness, HarnessTransport,
    TaskFrameKind, MEDULLA_TASK_PROTO,
};
use serde_json::json;

#[test]
fn encodes_a_minimal_frame() {
    let body = encode_task_frame(EncodeFrameInput {
        transport: None,
        kind: TaskFrameKind::Task,
        task_id: "cycle-1".to_string(),
        text: "do the thing".to_string(),
        ts: "2026-07-18T00:00:00.000Z".to_string(),
        correlation_id: None,
        harness: None,
        provider: None,
        custom_harness: None,
        model: None,
        tool_mode: None,
        workflow: None,
        workflow_fingerprint: None,
        workflow_inputs: Default::default(),
        conversation: None,
        fleet_depth: 0,
    });
    let value: serde_json::Value = serde_json::from_str(&body).unwrap();
    assert_eq!(value["proto"], MEDULLA_TASK_PROTO);
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
fn rejects_a_fleet_depth_that_cannot_fit_the_protocol_type() {
    let body = json!({
        "proto": MEDULLA_TASK_PROTO,
        "kind": "task",
        "taskId": "cycle-1",
        "text": "do the thing",
        "ts": "2026-07-18T00:00:00.000Z",
        "fleet_depth": 256,
    })
    .to_string();

    assert!(decode_task_frame(&body).is_none());
}

#[test]
fn encodes_optional_fields_when_present() {
    let body = encode_task_frame(EncodeFrameInput {
        transport: None,
        kind: TaskFrameKind::CapabilitiesResult,
        task_id: "t".to_string(),
        text: "{}".to_string(),
        ts: "2026-07-18T00:00:00.000Z".to_string(),
        correlation_id: Some("corr-9".to_string()),
        harness: Some(HarnessProvider::Codex),
        provider: Some(HarnessProvider::Claude),
        custom_harness: Some("deepseek-claude".into()),
        model: Some("anthropic/claude-opus-4.8".to_string()),
        tool_mode: None,
        workflow: Some("nightly-sweep".to_string()),
        workflow_fingerprint: Some("nightly-fingerprint".to_string()),
        workflow_inputs: json!({ "repo": "acme/api", "depth": 3 })
            .as_object()
            .unwrap()
            .clone(),
        conversation: None,
        fleet_depth: 0,
    });
    let value: serde_json::Value = serde_json::from_str(&body).unwrap();
    assert_eq!(value["kind"], "capabilities_result");
    assert_eq!(value["correlationId"], "corr-9");
    assert_eq!(value["harness"], "codex");
    assert_eq!(value["provider"], "claude");
    assert_eq!(value["model"], "anthropic/claude-opus-4.8");
    assert_eq!(value["workflow"], "nightly-sweep");
    assert_eq!(value["workflowFingerprint"], "nightly-fingerprint");
    assert_eq!(value["inputs"]["repo"], "acme/api");
    let decoded = decode_task_frame(&body).expect("the full frame decodes");
    assert_eq!(
        decoded.workflow_fingerprint.as_deref(),
        Some("nightly-fingerprint")
    );
    assert_eq!(decoded.workflow_inputs["depth"], json!(3));
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
            transport: None,
            kind,
            task_id: "t".to_string(),
            text: "x".to_string(),
            ts: "ts".to_string(),
            correlation_id: None,
            harness: None,
            provider: None,
            custom_harness: None,
            model: None,
            tool_mode: None,
            workflow: None,
            workflow_fingerprint: None,
            workflow_inputs: Default::default(),
            conversation: None,
            fleet_depth: 0,
        });
        let decoded = decode_task_frame(&body).expect("valid frame decodes");
        assert_eq!(decoded.kind, kind);
        assert_eq!(decoded.kind.as_str(), wire);
    }
}

#[test]
fn decodes_a_full_frame() {
    let body = json!({
        "proto": MEDULLA_TASK_PROTO,
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
        transport: None,
        kind: TaskFrameKind::Task,
        task_id: "t".to_string(),
        text: "x".to_string(),
        ts: "ts".to_string(),
        correlation_id: None,
        harness: None,
        provider: None,
        custom_harness: None,
        model: Some("openrouter/some-model".to_string()),
        tool_mode: None,
        workflow: None,
        workflow_fingerprint: None,
        workflow_inputs: Default::default(),
        conversation: None,
        fleet_depth: 0,
    });
    let decoded = decode_task_frame(&body).unwrap();
    assert_eq!(decoded.model.as_deref(), Some("openrouter/some-model"));
}

#[test]
fn decode_treats_absent_or_blank_model_as_none() {
    // Absent entirely.
    let absent = json!({
        "proto": MEDULLA_TASK_PROTO, "kind": "task", "taskId": "t", "text": "x", "ts": "ts",
    })
    .to_string();
    assert_eq!(decode_task_frame(&absent).unwrap().model, None);
    // Present but blank — treated as no hint so the daemon keeps its default.
    let blank = json!({
        "proto": MEDULLA_TASK_PROTO, "kind": "task", "taskId": "t", "text": "x", "ts": "ts",
        "model": "   ",
    })
    .to_string();
    assert_eq!(decode_task_frame(&blank).unwrap().model, None);
}

#[test]
fn decode_tolerates_missing_ts() {
    let body = json!({
        "proto": MEDULLA_TASK_PROTO,
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
        "proto": MEDULLA_TASK_PROTO,
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
        &json!({"proto": MEDULLA_TASK_PROTO, "kind": "nope", "taskId": "t", "text": "x"})
            .to_string()
    )
    .is_none());
    // Missing required text.
    assert!(decode_task_frame(
        &json!({"proto": MEDULLA_TASK_PROTO, "kind": "task", "taskId": "t"}).to_string()
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
    let caps = crate::protocol::AgentCapabilities {
        cwd: Some("/repo".to_string()),
        providers: vec![HarnessProvider::Claude],
        ..Default::default()
    };
    let value = serde_json::to_value(&caps).unwrap();
    assert!(value.get("budgets").is_none());
    assert!(value.get("readiness").is_none());
    assert!(value.get("screenKill").is_none());
}

#[test]
fn new_capabilities_round_trip_budgets_and_readiness() {
    let caps = crate::protocol::AgentCapabilities {
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
fn custom_harness_adverts_round_trip_without_execution_or_credential_details() {
    let caps = crate::protocol::AgentCapabilities {
        custom_harnesses: vec![crate::protocol::CustomHarnessAdvert {
            id: "deepseek".into(),
            name: "DeepSeek via Claude".into(),
            base_harness: HarnessProvider::Claude,
            model: "deepseek/deepseek-chat".into(),
            default: false,
        }],
        ..Default::default()
    };

    let value = serde_json::to_value(&caps).expect("serialize capabilities");
    let advert = &value["customHarnesses"][0];
    assert_eq!(advert["id"], "deepseek");
    assert_eq!(advert["baseHarness"], "claude");
    assert!(advert.get("apiKeyEnv").is_none());
    assert!(advert.get("baseUrl").is_none());
    assert_eq!(
        serde_json::from_value::<crate::protocol::AgentCapabilities>(value)
            .expect("parse capabilities"),
        caps
    );
}

/// A response says which session served the task. Reported, never requested:
/// this is the only path by which a session id travels back to a caller, which
/// otherwise has no way to name where its work happened.
#[test]
fn a_response_reports_the_session_that_served_the_task() {
    let body = crate::protocol::encode_task_frame_with_attachments(
        reply_input(),
        crate::protocol::FrameAttachments {
            session_id: Some("sess-42".to_string()),
            ..Default::default()
        },
    );
    let value: serde_json::Value = serde_json::from_str(&body).unwrap();
    assert_eq!(value["sessionId"], "sess-42");

    let decoded = decode_task_frame(&body).expect("a valid frame");
    assert_eq!(decoded.session_id.as_deref(), Some("sess-42"));
}

/// Blank is not a session, in either direction: an encoder that always writes
/// the key must not claim `""`, and a peer that sends one must not have it
/// recorded as a session id nothing can resume.
#[test]
fn a_blank_session_id_is_absent_rather_than_empty() {
    let body = crate::protocol::encode_task_frame_with_attachments(
        reply_input(),
        crate::protocol::FrameAttachments {
            session_id: Some("   ".to_string()),
            ..Default::default()
        },
    );
    let value: serde_json::Value = serde_json::from_str(&body).unwrap();
    assert!(value.get("sessionId").is_none());

    let sent_blank = json!({
        "proto": MEDULLA_TASK_PROTO,
        "kind": "reply",
        "taskId": "cycle-1",
        "text": "done",
        "ts": "2026-07-18T00:00:00.000Z",
        "sessionId": "  ",
    })
    .to_string();
    assert_eq!(decode_task_frame(&sent_blank).unwrap().session_id, None);
}

/// An ordinary outbound task never claims a session. Session *targeting* is a
/// separate feature with a separate trust story — one caller must not be able to
/// run inside a session opened for another — so the key stays off the request
/// side entirely.
#[test]
fn a_dispatched_task_names_no_session() {
    let body = encode_task_frame(EncodeFrameInput {
        kind: TaskFrameKind::Task,
        task_id: "cycle-1".to_string(),
        text: "do the thing".to_string(),
        ts: "2026-07-18T00:00:00.000Z".to_string(),
        correlation_id: None,
        harness: None,
        provider: None,
        transport: None,
        custom_harness: None,
        model: None,
        tool_mode: None,
        workflow: None,
        workflow_fingerprint: None,
        workflow_inputs: Default::default(),
        conversation: None,
        fleet_depth: 0,
    });
    let value: serde_json::Value = serde_json::from_str(&body).unwrap();
    assert!(value.get("sessionId").is_none());
}

/// A `reply` frame's inputs, for the session-reporting cases.
fn reply_input() -> EncodeFrameInput {
    EncodeFrameInput {
        kind: TaskFrameKind::Reply,
        task_id: "cycle-1".to_string(),
        text: "done".to_string(),
        ts: "2026-07-18T00:00:00.000Z".to_string(),
        correlation_id: None,
        harness: None,
        provider: None,
        transport: None,
        custom_harness: None,
        model: None,
        tool_mode: None,
        workflow: None,
        workflow_fingerprint: None,
        workflow_inputs: Default::default(),
        conversation: None,
        fleet_depth: 0,
    }
}

/// The flavor has to survive the wire, or a worker runs the CLI for a sender who
/// asked for the shared process and neither end can tell.
#[test]
fn carries_a_non_default_transport_across_the_wire() {
    let body = encode_task_frame(EncodeFrameInput {
        transport: Some(HarnessTransport::AppServer),
        kind: TaskFrameKind::Task,
        task_id: "t".to_string(),
        text: "do it".to_string(),
        ts: "2026-07-18T00:00:00.000Z".to_string(),
        correlation_id: None,
        harness: None,
        provider: Some(HarnessProvider::Codex),
        custom_harness: None,
        model: None,
        tool_mode: None,
        workflow: None,
        workflow_fingerprint: None,
        workflow_inputs: Default::default(),
        conversation: None,
        fleet_depth: 0,
    });
    let value: serde_json::Value = serde_json::from_str(&body).expect("valid json");
    assert_eq!(value["transport"], "app_server");

    let decoded = decode_task_frame(&body).expect("decodes");
    assert_eq!(decoded.transport, Some(HarnessTransport::AppServer));
}

/// An ordinary task must be byte-identical to what a peer that predates flavors
/// would send, so the key is dropped when it says nothing.
#[test]
fn omits_the_default_transport_from_the_wire() {
    for transport in [None, Some(HarnessTransport::Cli)] {
        let body = encode_task_frame(EncodeFrameInput {
            transport,
            kind: TaskFrameKind::Task,
            task_id: "t".to_string(),
            text: "do it".to_string(),
            ts: "2026-07-18T00:00:00.000Z".to_string(),
            correlation_id: None,
            harness: None,
            provider: Some(HarnessProvider::Codex),
            custom_harness: None,
            model: None,
            tool_mode: None,
            workflow: None,
            workflow_fingerprint: None,
            workflow_inputs: Default::default(),
            conversation: None,
            fleet_depth: 0,
        });
        let value: serde_json::Value = serde_json::from_str(&body).expect("valid json");
        assert!(value.get("transport").is_none(), "{transport:?}: {body}");
        assert_eq!(decode_task_frame(&body).unwrap().transport, None);
    }
}

/// Failing closed, not open: a worker that ran the CLI because it did not
/// understand the flavor is a silent downgrade the sender cannot detect.
#[test]
fn refuses_a_frame_naming_a_transport_it_does_not_understand() {
    for bad in [json!("quantum"), json!(7), json!({})] {
        let body = json!({
            "proto": MEDULLA_TASK_PROTO,
            "kind": "task",
            "taskId": "t",
            "text": "do it",
            "ts": "2026-07-18T00:00:00.000Z",
            "transport": bad,
        })
        .to_string();
        assert!(decode_task_frame(&body).is_none(), "{bad}");
    }
}
