//! Tests for the backend-shaped capability payload.

use super::*;
use crate::protocol::{
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
    let payload = capabilities_payload("claude", Some(&probed()), None, &[]);
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
    let payload = capabilities_payload("codex", None, None, &[]);
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
        capabilities_payload("claude", Some(&caps), None, &[])["summary"],
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
    let payload = capabilities_payload("claude", Some(&caps), None, &[]);
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
    let payload = capabilities_payload("claude", Some(&caps), None, &[]);
    assert_eq!(payload["providers"], serde_json::json!(["codex", "claude"]));
}

#[test]
fn custom_harnesses_reach_the_backend_capability_payload() {
    let mut caps = probed();
    caps.custom_harnesses = vec![crate::protocol::CustomHarnessAdvert {
        id: "deepseek".into(),
        name: "DeepSeek via Claude".into(),
        base_harness: HarnessProvider::Claude,
        model: "deepseek/deepseek-chat".into(),
        default: false,
    }];

    let payload = capabilities_payload("claude", Some(&caps), None, &[]);

    assert_eq!(payload["customHarnesses"][0]["id"], "deepseek");
    assert!(payload["customHarnesses"][0].get("apiKeyEnv").is_none());
}

#[test]
fn a_nameless_harness_advertises_no_providers_rather_than_one_called_nothing() {
    let payload = capabilities_payload("  ", None, None, &[]);
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
    let payload = capabilities_payload("claude", Some(&caps), None, &[]);
    assert_eq!(payload["ready"], false);
    assert_eq!(payload["readyReason"], "rate limited");
    assert_eq!(payload["harnessBudgets"][0]["seat"], "seat-1");
    // …alongside, not instead of, the context.
    assert_eq!(payload["project"], "medulla");
}

#[test]
fn a_roles_tool_allowlist_narrows_what_the_probe_reported() {
    // The probe reports what the *harness* has; a role says what the worker is
    // *allowed*. Reporting the first unfiltered made the two channels
    // contradict each other about one worker — the descriptor saying "reviews
    // diffs" while capabilities claimed it could edit files.
    let mut caps = probed();
    caps.tools = vec![
        "read".to_string(),
        "search".to_string(),
        "edit".to_string(),
        "shell".to_string(),
    ];
    let allowed = [
        "read".to_string(),
        "search".to_string(),
        "shell".to_string(),
    ];

    let payload = capabilities_payload("claude", Some(&caps), Some(&allowed), &[]);
    let tools: Vec<&str> = payload["tools"]
        .as_array()
        .expect("tools")
        .iter()
        .map(|t| t.as_str().expect("a tool"))
        .collect();

    assert_eq!(tools, vec!["read", "search", "shell"]);
    assert!(!tools.contains(&"edit"), "the role does not permit editing");
}

#[test]
fn a_worker_with_no_role_allowlist_reports_every_tool_it_has() {
    // "Unconstrained" and "allowed nothing" must stay distinct: an unspecified
    // worker advertising no tools would make declaring roles compulsory.
    let caps = probed();
    let expected = caps.tools.len();
    let payload = capabilities_payload("claude", Some(&caps), None, &[]);
    assert_eq!(payload["tools"].as_array().expect("tools").len(), expected);
}

#[test]
fn assigned_roles_travel_with_the_capabilities() {
    let roles = ["code-reviewer".to_string()];
    let payload = capabilities_payload("claude", Some(&probed()), None, &roles);
    assert_eq!(payload["roles"][0], "code-reviewer");

    // Absent rather than empty when none are set: an absent key reads as "not
    // specified", where `[]` reads as "deliberately none".
    let payload = capabilities_payload("claude", Some(&probed()), None, &[]);
    assert!(payload.get("roles").is_none(), "{payload}");
}

#[test]
fn a_worker_that_resolves_none_of_its_roles_advertises_no_tools() {
    // Fail-closed. `filter_map` used to drop unknown ids and leave the worker
    // unconstrained, so a typo in a role id — the one case where the operator
    // meant to *narrow* what a machine does — silently advertised every tool the
    // harness has instead.
    let catalog = crate::agents::default_templates();
    let allowed = super::role_tool_allowlist(&["typoed-role".to_string()], &catalog);
    assert_eq!(
        allowed,
        Some(Vec::new()),
        "an unresolvable role must deny, not open"
    );
}

#[test]
fn an_empty_catalog_leaves_a_worker_unconstrained() {
    // The other half of the rule: no catalog means the templates have not been
    // read, not that every id is bad. Failing closed here would take a whole
    // fleet offline on a transient read.
    let allowed = super::role_tool_allowlist(&["code-reviewer".to_string()], &[]);
    assert_eq!(allowed, None);
}

#[test]
fn a_worker_naming_no_roles_stays_unconstrained() {
    let catalog = crate::agents::default_templates();
    assert_eq!(super::role_tool_allowlist(&[], &catalog), None);
}

#[test]
fn resolved_roles_union_their_allowlists() {
    let catalog = crate::agents::default_templates();
    let allowed = super::role_tool_allowlist(
        &["code-reviewer".to_string(), "typoed-role".to_string()],
        &catalog,
    )
    .expect("a resolved role constrains");
    // The unresolvable id contributes nothing, but it no longer opens the gate:
    // one role resolved, so the constraint is that role's allowlist.
    assert!(allowed.contains(&"read".to_string()), "{allowed:?}");
    assert!(!allowed.contains(&"edit".to_string()), "{allowed:?}");
}
