//! Tests for the fail-open budget/readiness probe core and its seam driver.

use std::collections::HashMap;

use crate::tinyplace::{BudgetSource, BudgetWindow, HarnessProvider};

use super::{evaluate_provider, probe_budgets, BudgetSeams, ConfiguredBudget, ProviderProbeInput};

const NOW: i64 = 1_000_000;

fn input(provider: HarnessProvider) -> ProviderProbeInput {
    ProviderProbeInput {
        provider,
        installed: true,
        authenticated: None,
        cooldown_until: None,
        configured: None,
        now_unix: NOW,
    }
}

#[test]
fn installed_unknown_auth_is_ready_with_an_estimate() {
    let (readiness, budget) = evaluate_provider(&input(HarnessProvider::Claude));
    assert!(readiness.ready, "unknown auth fails open to ready");
    assert!(readiness.reason.is_none());
    let budget = budget.expect("a usable seat advertises an estimate");
    assert_eq!(budget.provider, HarnessProvider::Claude);
    assert_eq!(budget.source, BudgetSource::Estimate);
    assert_eq!(budget.window, BudgetWindow::Unknown);
    // No invented numbers.
    assert!(budget.limit_tokens.is_none());
    assert!(budget.used_tokens.is_none());
    assert!(budget.remaining_tokens.is_none());
    assert!(budget.seat.is_none());
}

#[test]
fn not_installed_yields_no_budget() {
    let mut probe = input(HarnessProvider::Codex);
    probe.installed = false;
    let (readiness, budget) = evaluate_provider(&probe);
    assert!(!readiness.ready);
    assert_eq!(readiness.reason.as_deref(), Some("not installed"));
    assert!(budget.is_none());
}

#[test]
fn definitively_unauthenticated_is_not_ready_and_has_no_estimate() {
    let mut probe = input(HarnessProvider::Codex);
    probe.authenticated = Some(false);
    let (readiness, budget) = evaluate_provider(&probe);
    assert!(!readiness.ready);
    assert_eq!(readiness.reason.as_deref(), Some("not authenticated"));
    // No usable capacity to advertise, and no configured numbers → no budget.
    assert!(budget.is_none());
}

#[test]
fn active_cooldown_marks_not_ready() {
    let mut probe = input(HarnessProvider::Claude);
    probe.cooldown_until = Some(NOW + 500);
    let (readiness, budget) = evaluate_provider(&probe);
    assert!(!readiness.ready);
    assert_eq!(
        readiness.reason.as_deref(),
        Some("in cooldown until 1000500")
    );
    // Still a seat, so an estimate carrying the cooldown is emitted.
    let budget = budget.expect("estimate with cooldown");
    assert_eq!(budget.cooldown_until, Some(NOW + 500));
}

#[test]
fn past_cooldown_is_ready() {
    let mut probe = input(HarnessProvider::Claude);
    probe.cooldown_until = Some(NOW - 500);
    let (readiness, _) = evaluate_provider(&probe);
    assert!(readiness.ready, "an expired cooldown does not block");
}

#[test]
fn configured_numbers_are_authoritative_with_clamped_remaining() {
    let mut probe = input(HarnessProvider::Opencode);
    probe.configured = Some(ConfiguredBudget {
        seat: Some("team-seat".to_string()),
        window: Some(BudgetWindow::Weekly),
        limit_tokens: Some(1_000),
        used_tokens: Some(1_200),
        remaining_tokens: None,
        cooldown_until: None,
    });
    let (_, budget) = evaluate_provider(&probe);
    let budget = budget.expect("configured budget");
    assert_eq!(budget.source, BudgetSource::Configured);
    assert_eq!(budget.window, BudgetWindow::Weekly);
    assert_eq!(budget.seat.as_deref(), Some("team-seat"));
    assert_eq!(budget.limit_tokens, Some(1_000));
    assert_eq!(budget.used_tokens, Some(1_200));
    // limit - used, clamped at zero.
    assert_eq!(budget.remaining_tokens, Some(0));
}

#[test]
fn configured_remaining_absent_without_both_inputs() {
    let mut probe = input(HarnessProvider::Codex);
    probe.configured = Some(ConfiguredBudget {
        limit_tokens: Some(5_000),
        ..Default::default()
    });
    let (_, budget) = evaluate_provider(&probe);
    let budget = budget.expect("configured budget");
    assert_eq!(budget.limit_tokens, Some(5_000));
    assert!(budget.remaining_tokens.is_none());
}

#[test]
fn configured_cooldown_overrides_and_marks_not_ready() {
    let mut probe = input(HarnessProvider::Codex);
    probe.configured = Some(ConfiguredBudget {
        cooldown_until: Some(NOW + 10),
        ..Default::default()
    });
    let (readiness, budget) = evaluate_provider(&probe);
    assert!(!readiness.ready);
    assert_eq!(
        budget.expect("configured budget").cooldown_until,
        Some(NOW + 10)
    );
}

#[test]
fn configured_budget_survives_unconfirmed_auth() {
    // Operator numbers are authoritative even when auth cannot be confirmed.
    let mut probe = input(HarnessProvider::Codex);
    probe.authenticated = Some(false);
    probe.configured = Some(ConfiguredBudget {
        limit_tokens: Some(100),
        used_tokens: Some(40),
        ..Default::default()
    });
    let (readiness, budget) = evaluate_provider(&probe);
    assert!(!readiness.ready, "auth signal still governs readiness");
    let budget = budget.expect("configured budget despite auth");
    assert_eq!(budget.remaining_tokens, Some(60));
    assert_eq!(budget.source, BudgetSource::Configured);
}

// --- driver over injected seams ---

fn seams(
    installed: Vec<HarnessProvider>,
    configured: Option<(HarnessProvider, ConfiguredBudget)>,
) -> BudgetSeams {
    let installed_set = installed;
    let configured_pair = configured;
    BudgetSeams {
        installed: Box::new(move |p| installed_set.contains(&p)),
        authenticated: Box::new(|_| None),
        cooldown_until: Box::new(|_| None),
        configured: Box::new(move |p| {
            configured_pair
                .as_ref()
                .filter(|(cp, _)| *cp == p)
                .map(|(_, cfg)| cfg.clone())
        }),
        now_unix: Box::new(|| NOW),
    }
}

#[test]
fn driver_omits_absent_providers_and_reports_installed_ones() {
    let seams = seams(vec![HarnessProvider::Claude], None);
    let (readiness, budgets) =
        probe_budgets(&[HarnessProvider::Claude, HarnessProvider::Codex], &seams);
    // Only the installed provider appears; the absent one is omitted entirely.
    assert_eq!(readiness.len(), 1);
    assert_eq!(readiness[0].provider, HarnessProvider::Claude);
    assert!(readiness[0].ready);
    assert_eq!(budgets.len(), 1);
    assert_eq!(budgets[0].provider, HarnessProvider::Claude);
    assert_eq!(budgets[0].source, BudgetSource::Estimate);
}

#[test]
fn driver_threads_configured_numbers() {
    let cfg = ConfiguredBudget {
        limit_tokens: Some(2_000),
        used_tokens: Some(500),
        window: Some(BudgetWindow::Daily),
        ..Default::default()
    };
    let seams = seams(
        vec![HarnessProvider::Codex],
        Some((HarnessProvider::Codex, cfg)),
    );
    let (_, budgets) = probe_budgets(&[HarnessProvider::Codex], &seams);
    assert_eq!(budgets.len(), 1);
    assert_eq!(budgets[0].source, BudgetSource::Configured);
    assert_eq!(budgets[0].remaining_tokens, Some(1_500));
    assert_eq!(budgets[0].window, BudgetWindow::Daily);
}

#[test]
fn with_configured_promotes_matching_provider_from_config() {
    // The `[budget]` config seam: a configured provider advertises
    // `source: configured`; a provider absent from the config keeps its estimate.
    use crate::config::{BudgetConfig, ProviderBudgetConfig};

    let mut providers = HashMap::new();
    providers.insert(
        "codex".to_string(),
        ProviderBudgetConfig {
            window: Some(BudgetWindow::Weekly),
            limit_tokens: Some(9_000),
            used_tokens: Some(1_000),
            ..Default::default()
        },
    );
    let budget = BudgetConfig { providers };

    // Start from an all-installed, no-configured seam, then layer the config on.
    let base = seams(vec![HarnessProvider::Codex, HarnessProvider::Claude], None);
    let seams = base.with_configured(budget);
    let (_, budgets) = probe_budgets(&[HarnessProvider::Codex, HarnessProvider::Claude], &seams);

    let codex = budgets
        .iter()
        .find(|b| b.provider == HarnessProvider::Codex)
        .expect("codex budget");
    assert_eq!(codex.source, BudgetSource::Configured);
    assert_eq!(codex.window, BudgetWindow::Weekly);
    assert_eq!(codex.remaining_tokens, Some(8_000));

    let claude = budgets
        .iter()
        .find(|b| b.provider == HarnessProvider::Claude)
        .expect("claude budget");
    assert_eq!(
        claude.source,
        BudgetSource::Estimate,
        "an unconfigured provider stays on an estimate"
    );
}

#[test]
fn configured_from_config_prefers_explicit_remaining() {
    // A ProviderBudgetConfig with an explicit remaining wins over the derivation.
    use crate::config::ProviderBudgetConfig;
    let cfg = ConfiguredBudget::from_config(&ProviderBudgetConfig {
        limit_tokens: Some(1_000),
        used_tokens: Some(100),
        remaining_tokens: Some(7),
        ..Default::default()
    });
    let mut probe = input(HarnessProvider::Opencode);
    probe.configured = Some(cfg);
    let (_, budget) = evaluate_provider(&probe);
    let budget = budget.expect("configured budget");
    assert_eq!(
        budget.remaining_tokens,
        Some(7),
        "explicit remaining beats limit - used"
    );
}

#[test]
fn from_env_reports_nothing_without_installed_binaries() {
    // Empty env → empty PATH → nothing resolves → no fleet advertised.
    let seams = BudgetSeams::from_env(&HashMap::new());
    let (readiness, budgets) =
        probe_budgets(&[HarnessProvider::Claude, HarnessProvider::Codex], &seams);
    assert!(readiness.is_empty());
    assert!(budgets.is_empty());
}
