//! Tests for credential rows, account grouping, and usage formatting.

use medulla::runtime::{CapacitySnapshot, HarnessBudget, HarnessDescriptor, HostDescriptor};

use super::{credential_line, grouping::account_groups, usage::usage_summary};

#[test]
fn credential_rows_distinguish_connected_and_missing_sources() {
    let connected = credential_line("Codex", true, "unused");
    let missing = credential_line("OpenAI", false, "set the environment variable");

    let connected_text = connected
        .spans
        .iter()
        .map(|span| span.content.as_ref())
        .collect::<String>();
    let missing_text = missing
        .spans
        .iter()
        .map(|span| span.content.as_ref())
        .collect::<String>();

    assert!(connected_text.contains("● Codex"));
    assert!(connected_text.contains("connected"));
    assert!(missing_text.contains("○ OpenAI"));
    assert!(missing_text.contains("set the environment variable"));
}

#[test]
fn two_claude_accounts_stay_separate_and_one_account_collects_its_hosts() {
    let capacity = CapacitySnapshot {
        hosts: vec![host("laptop"), host("builder"), host("cloud")],
        harnesses: vec![
            harness("claude-laptop", "laptop", "account-a", "weekly", 80),
            harness("claude-builder", "builder", "account-a", "daily", 55),
            harness("claude-cloud", "cloud", "account-b", "fable", 25),
        ],
        ..Default::default()
    };

    let groups = account_groups(&capacity, "claude-code");
    assert_eq!(groups.len(), 2);
    assert_eq!(groups[0].account_id.as_deref(), Some("account-a"));
    assert_eq!(groups[0].harnesses.len(), 2);
    assert_eq!(groups[0].budgets.len(), 2);
    assert_eq!(groups[1].account_id.as_deref(), Some("account-b"));
    assert_eq!(groups[1].harnesses.len(), 1);
}

#[test]
fn usage_keeps_every_window_and_formats_openrouter_usd_balance() {
    let daily = token_budget("daily", 800, 1_000);
    let weekly = token_budget("weekly", 4_000, 10_000);
    let fable = token_budget("fable", 250, 1_000);
    let balance = HarnessBudget {
        provider: "openrouter".into(),
        window: "balance".into(),
        seat: Some("key-work".into()),
        account_type: Some("api_key".into()),
        limit_tokens: None,
        used_tokens: None,
        remaining_tokens: None,
        amount_unit: Some("USD".into()),
        limit_amount: Some(50.0),
        used_amount: Some(31.58),
        remaining_amount: Some(18.42),
        cooldown_until: None,
        source: "provider_reported".into(),
    };

    let summary = usage_summary(&[&daily, &weekly, &fable, &balance]).unwrap();
    assert!(summary.contains("daily · 800 tokens left (80%)"));
    assert!(summary.contains("weekly · 4k tokens left (40%)"));
    assert!(summary.contains("fable · 250 tokens left (25%)"));
    assert!(summary.contains("balance · $18.42 left (37%)"));
}

fn host(name: &str) -> HostDescriptor {
    HostDescriptor {
        id: name.into(),
        name: name.into(),
        availability: "online".into(),
        address: None,
        resources: None,
        metadata: Default::default(),
    }
}

fn harness(
    id: &str,
    host_id: &str,
    account_id: &str,
    window: &str,
    remaining: u64,
) -> HarnessDescriptor {
    HarnessDescriptor {
        id: id.into(),
        host_id: host_id.into(),
        kind: "claude-code".into(),
        availability: "online".into(),
        ready: true,
        ready_reason: None,
        providers: vec!["anthropic".into()],
        template_ids: Vec::new(),
        budgets: vec![HarnessBudget {
            seat: Some(account_id.into()),
            account_type: Some("subscription".into()),
            ..token_budget(window, remaining, 100)
        }],
        metadata: Default::default(),
    }
}

fn token_budget(window: &str, remaining: u64, limit: u64) -> HarnessBudget {
    HarnessBudget {
        provider: "anthropic".into(),
        window: window.into(),
        seat: None,
        account_type: None,
        limit_tokens: Some(limit),
        used_tokens: Some(limit.saturating_sub(remaining)),
        remaining_tokens: Some(remaining),
        amount_unit: None,
        limit_amount: None,
        used_amount: None,
        remaining_amount: None,
        cooldown_until: None,
        source: "provider_reported".into(),
    }
}
