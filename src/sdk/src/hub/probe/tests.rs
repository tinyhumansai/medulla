//! Tests for the backend-shaped capability payload.

use super::*;
use crate::tinyplace::{
    BudgetSource, BudgetWindow, HarnessBudget, HarnessProvider, HarnessReadiness,
};

fn probed() -> AgentCapabilities {
    AgentCapabilities {
        cwd: Some("/srv/repos/medulla".to_string()),
        accessible_dirs: vec![
            "/srv/repos/medulla".to_string(),
            "/srv/repos/backend".to_string(),
        ],
        project: Some("medulla".to_string()),
        branch: Some("main".to_string()),
        providers: vec![HarnessProvider::Claude],
        tools: vec!["Bash".to_string(), "Read".to_string()],
        mcp_servers: vec!["github".to_string()],
        summary: Some("Rust workspace: the medulla SDK and its TUI.".to_string()),
        ..Default::default()
    }
}

#[test]
fn the_probes_context_reaches_the_backend_rather_than_being_dropped() {
    // The bug this module exists to fix: the orchestrator was told "claude
    // daemon" and nothing else, so it delegated with no idea what was in the
    // directory or which directories there even were.
    let payload = capabilities_payload("claude", Some(&probed()));
    assert_eq!(payload["cwd"], "/srv/repos/medulla");
    assert_eq!(payload["project"], "medulla");
    assert_eq!(payload["branch"], "main");
    assert_eq!(
        payload["summary"],
        "Rust workspace: the medulla SDK and its TUI."
    );
    assert_eq!(
        payload["accessibleDirs"],
        serde_json::json!(["/srv/repos/medulla", "/srv/repos/backend"])
    );
    assert_eq!(payload["tools"], serde_json::json!(["Bash", "Read"]));
    assert_eq!(payload["mcpServers"], serde_json::json!(["github"]));
    assert_eq!(payload["providers"], serde_json::json!(["claude"]));
}

#[test]
fn an_unreachable_worker_still_advertises_what_the_roster_knows() {
    // The probe fails open. Answering with nothing would drop the worker out of
    // every fan-out; answering with the configured harness keeps it delegable.
    let payload = capabilities_payload("codex", None);
    assert_eq!(payload["providers"], serde_json::json!(["codex"]));
    assert_eq!(payload["summary"], "codex daemon");
    assert!(payload.get("cwd").is_none(), "{payload}");
    assert!(payload.get("accessibleDirs").is_none(), "{payload}");
}

#[test]
fn a_real_summary_wins_over_the_synthetic_one() {
    let mut caps = probed();
    caps.summary = Some("  ".to_string());
    // A blank summary is not a summary; the placeholder is better than "".
    assert_eq!(
        capabilities_payload("claude", Some(&caps))["summary"],
        "claude daemon"
    );
}

#[test]
fn unreported_fields_are_omitted_rather_than_sent_blank() {
    // An absent key is "not reported". `""` reads as a measurement someone took.
    let caps = AgentCapabilities {
        cwd: Some("   ".to_string()),
        accessible_dirs: vec!["".to_string(), "  ".to_string()],
        ..Default::default()
    };
    let payload = capabilities_payload("claude", Some(&caps));
    assert!(payload.get("cwd").is_none(), "{payload}");
    assert!(payload.get("accessibleDirs").is_none(), "{payload}");
    assert!(payload.get("project").is_none(), "{payload}");
}

#[test]
fn the_probes_providers_win_over_the_configured_harness() {
    // The roster records one harness because that is what the operator typed;
    // the probe reports what is actually installed.
    let mut caps = probed();
    caps.providers = vec![HarnessProvider::Codex, HarnessProvider::Claude];
    let payload = capabilities_payload("claude", Some(&caps));
    assert_eq!(payload["providers"], serde_json::json!(["codex", "claude"]));
}

#[test]
fn a_nameless_harness_advertises_no_providers_rather_than_one_called_nothing() {
    let payload = capabilities_payload("  ", None);
    assert_eq!(payload["providers"], serde_json::json!([]));
    assert!(payload.get("summary").is_none(), "{payload}");
}

#[test]
fn budgets_and_readiness_still_ride_along() {
    let mut caps = probed();
    caps.budgets = vec![HarnessBudget {
        provider: HarnessProvider::Claude,
        seat: Some("seat-1".to_string()),
        window: BudgetWindow::FiveHour,
        limit_tokens: Some(1_000),
        used_tokens: Some(250),
        remaining_tokens: Some(750),
        cooldown_until: None,
        source: BudgetSource::Estimate,
    }];
    caps.readiness = vec![HarnessReadiness {
        provider: HarnessProvider::Claude,
        ready: false,
        reason: Some("rate limited".to_string()),
    }];
    let payload = capabilities_payload("claude", Some(&caps));
    assert_eq!(payload["ready"], false);
    assert_eq!(payload["readyReason"], "rate limited");
    assert_eq!(payload["harnessBudgets"][0]["seat"], "seat-1");
    // …alongside, not instead of, the context.
    assert_eq!(payload["project"], "medulla");
}
