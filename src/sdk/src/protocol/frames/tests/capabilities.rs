//! Compatibility tests for additive worker capability fields, and the
//! `AgentCapabilities` wire round-trips for budgets, readiness, and custom
//! harness adverts (moved from `codec.rs`; see AGENTS.md's 500-line ceiling).

use crate::protocol::{
    parse_agent_capabilities, AgentCapabilities, BudgetSource, BudgetWindow, HarnessBudget,
    HarnessProvider, HarnessReadiness,
};
use serde_json::json;

#[test]
fn budget_window_defaults_to_unknown() {
    assert_eq!(BudgetWindow::default(), BudgetWindow::Unknown);
}

#[test]
fn screen_kill_support_is_additive_and_defaults_off_for_older_workers() {
    let older = parse_agent_capabilities(r#"{"providers":["claude"]}"#).unwrap();
    assert!(!older.screen_kill);

    let current = AgentCapabilities {
        screen_kill: true,
        ..Default::default()
    };
    let value = serde_json::to_value(&current).unwrap();
    assert_eq!(value["screenKill"], true);
}

#[test]
fn as_wire_is_the_exact_inverse_of_from_wire() {
    // The two names a provider has are easy to mix up: `as_str` is the display
    // name (`OpenCode`) and does not parse back. Asserting the round trip here
    // means a new provider cannot be added with only half the pair.
    for provider in [
        HarnessProvider::Claude,
        HarnessProvider::Codex,
        HarnessProvider::Opencode,
        HarnessProvider::Openhuman,
        HarnessProvider::Shell,
    ] {
        assert_eq!(
            HarnessProvider::from_wire(provider.as_wire()),
            Some(provider),
            "{provider:?} does not survive as_wire → from_wire"
        );
    }
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
