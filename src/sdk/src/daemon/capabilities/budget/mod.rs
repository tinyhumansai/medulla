//! Best-effort, fail-open budget and readiness probe for the installed harnesses.
//!
//! An orchestrator sizing tasks wants to know, per accepted harness, how much
//! headroom a worker has and whether the harness is usable at all. Providers do
//! not publish exact per-window numbers, so this probe is honest about its
//! sourcing: it emits `source: estimate` for anything inferred and only reports
//! `source: configured` when an operator sets numbers explicitly. It never
//! invents an allowance, and it never blocks — a probe that cannot establish a
//! fact degrades to an absent field (or, for an unusable harness, a
//! present-but-not-ready readiness entry), not an error.
//!
//! The design separates a *pure core* ([`evaluate_provider`]) that maps already
//! gathered facts to a readiness/budget pair from the *environment seams*
//! ([`BudgetSeams`]) that gather those facts from the filesystem and process
//! environment. Tests drive the core with injected seams so nothing here reaches
//! the real machine. The data types live in `types`.
//!
//! Operator-configured budget numbers arrive through the [`BudgetSeams::configured`]
//! seam. [`BudgetSeams::with_configured`] backs it with the `[budget]` config
//! section (`config::BudgetConfig`); without a `[budget.providers.<p>]` entry the
//! seam returns `None` and every installed, usable harness advertises an
//! `estimate` with no invented numbers.

use std::collections::HashMap;
use std::path::PathBuf;

use crate::protocol::{
    BudgetSource, BudgetWindow, HarnessBudget, HarnessProvider, HarnessReadiness,
};

use super::super::providers::{make_path_lookup, provider_bin};

mod types;
pub use types::{BudgetSeams, ConfiguredBudget, ProviderProbeInput};

#[cfg(test)]
mod tests;

/// Evaluate one provider into its readiness and an optional budget descriptor.
///
/// Pure: every environment fact arrives via `input`. Fails open — an
/// unverifiable authentication state (`None`) leaves the harness `ready`, and a
/// harness with no configured numbers advertises an `estimate` with no invented
/// allowance rather than a fabricated one. A harness that is not installed yields
/// no budget (it is not part of the fleet); an installed harness that is
/// definitively unauthenticated is reported not-ready and, absent operator
/// numbers, carries no budget (there is no usable capacity to advertise).
pub fn evaluate_provider(input: &ProviderProbeInput) -> (HarnessReadiness, Option<HarnessBudget>) {
    // A configured cooldown overrides a detected one; either can park a seat.
    let cooldown_until = input
        .configured
        .as_ref()
        .and_then(|c| c.cooldown_until)
        .or(input.cooldown_until);
    let cooling = cooldown_until
        .map(|until| until > input.now_unix)
        .unwrap_or(false);

    let (ready, reason) = if !input.installed {
        (false, Some("not installed".to_string()))
    } else if input.authenticated == Some(false) {
        (false, Some("not authenticated".to_string()))
    } else if cooling {
        (
            false,
            Some(format!(
                "in cooldown until {}",
                cooldown_until.expect("cooling implies a cooldown time")
            )),
        )
    } else {
        (true, None)
    };
    let readiness = HarnessReadiness {
        provider: input.provider,
        ready,
        reason,
    };

    // Nothing to size against for a harness that is not on the machine.
    if !input.installed {
        return (readiness, None);
    }

    let budget = match &input.configured {
        // Operator-declared numbers are authoritative even if the auth heuristic
        // could not confirm the seat.
        Some(cfg) => Some(HarnessBudget {
            provider: input.provider,
            seat: cfg.seat.clone(),
            window: cfg.window.unwrap_or(BudgetWindow::Unknown),
            limit_tokens: cfg.limit_tokens,
            used_tokens: cfg.used_tokens,
            // The operator's explicit remaining wins; otherwise derive it from
            // limit - used when both are known, clamped at zero.
            remaining_tokens: cfg
                .remaining_tokens
                .or(match (cfg.limit_tokens, cfg.used_tokens) {
                    (Some(limit), Some(used)) => Some((limit - used).max(0)),
                    _ => None,
                }),
            cooldown_until,
            source: BudgetSource::Configured,
        }),
        // No configured numbers: advertise an honest estimate for a usable seat,
        // but nothing for a definitively unauthenticated one (no invented budget).
        None if input.authenticated != Some(false) => Some(HarnessBudget {
            provider: input.provider,
            seat: None,
            window: BudgetWindow::Unknown,
            limit_tokens: None,
            used_tokens: None,
            remaining_tokens: None,
            cooldown_until,
            source: BudgetSource::Estimate,
        }),
        None => None,
    };
    (readiness, budget)
}

impl BudgetSeams {
    /// Wire the seams against the real machine, using the controlled spawn `env`
    /// for binary resolution and authentication env-var checks.
    ///
    /// Fails open throughout: a missing `PATH`, absent credential file, or unset
    /// env var yields "not installed" / "unknown auth", never an error. The
    /// cooldown seam has no real source yet and returns `None`; the configured
    /// seam defaults to `None` too and is populated by [`Self::with_configured`]
    /// when an operator sets a `[budget]` section.
    pub fn from_env(env: &HashMap<String, String>) -> Self {
        let lookup = make_path_lookup(env);
        let env_for_bin = env.clone();
        let env_for_auth = env.clone();
        BudgetSeams {
            installed: Box::new(move |provider| lookup(&provider_bin(provider, &env_for_bin))),
            authenticated: Box::new(move |provider| {
                provider_authenticated(provider, &env_for_auth)
            }),
            cooldown_until: Box::new(|_| None),
            configured: Box::new(|_| None),
            now_unix: Box::new(now_unix_seconds),
        }
    }

    /// Replace the `configured` seam with one backed by the operator's `[budget]`
    /// config section, so a `[budget.providers.<p>]` entry promotes that
    /// provider's advertised descriptor to `source: configured`. Providers absent
    /// from the config keep returning `None` (an honest estimate).
    pub fn with_configured(mut self, budget: crate::config::BudgetConfig) -> Self {
        self.configured = Box::new(move |provider| {
            budget
                .for_provider(provider.as_str())
                .map(ConfiguredBudget::from_config)
        });
        self
    }
}

/// Probe every offered provider, returning readiness for the installed ones and
/// budgets for those with capacity to advertise.
///
/// A provider absent from the machine is simply not part of the fleet, so it is
/// omitted entirely; an installed-but-unusable provider is reported present with
/// `ready = false` so a consumer avoids it. Never fails.
pub fn probe_budgets(
    providers: &[HarnessProvider],
    seams: &BudgetSeams,
) -> (Vec<HarnessReadiness>, Vec<HarnessBudget>) {
    let now = (seams.now_unix)();
    let mut readiness = Vec::new();
    let mut budgets = Vec::new();
    for &provider in providers {
        if !(seams.installed)(provider) {
            continue;
        }
        let input = ProviderProbeInput {
            provider,
            installed: true,
            authenticated: (seams.authenticated)(provider),
            cooldown_until: (seams.cooldown_until)(provider),
            configured: (seams.configured)(provider),
            now_unix: now,
        };
        let (entry, budget) = evaluate_provider(&input);
        readiness.push(entry);
        if let Some(budget) = budget {
            budgets.push(budget);
        }
    }
    (readiness, budgets)
}

/// Best-effort authentication heuristic for a provider.
///
/// Returns `Some(true)` on the first positive signal — a non-empty known auth
/// env var in the controlled spawn `env`, or an existing credential file in the
/// user's home — and `None` when nothing is found. It never returns `Some(false)`:
/// the absence of a signal is "unknown", not "logged out", so readiness fails
/// open. Credential contents are never read; only presence is checked, so no seat
/// identifier or key can leak.
fn provider_authenticated(
    provider: HarnessProvider,
    env: &HashMap<String, String>,
) -> Option<bool> {
    let env_signal = auth_env_vars(provider)
        .iter()
        .any(|key| env.get(*key).map(|v| !v.trim().is_empty()).unwrap_or(false));
    if env_signal {
        return Some(true);
    }
    if auth_files(provider).iter().any(|path| path.exists()) {
        return Some(true);
    }
    None
}

/// Known authentication env-var names per provider (presence, not contents).
fn auth_env_vars(provider: HarnessProvider) -> &'static [&'static str] {
    match provider {
        HarnessProvider::Claude => &[
            "ANTHROPIC_API_KEY",
            "ANTHROPIC_AUTH_TOKEN",
            "CLAUDE_CODE_OAUTH_TOKEN",
        ],
        HarnessProvider::Codex => &["OPENAI_API_KEY", "CODEX_API_KEY"],
        HarnessProvider::Opencode => &["OPENCODE_API_KEY"],
    }
}

/// Well-known credential file paths per provider, under the user's home.
fn auth_files(provider: HarnessProvider) -> Vec<PathBuf> {
    let Some(home) = home_dir() else {
        return Vec::new();
    };
    match provider {
        HarnessProvider::Claude => vec![
            home.join(".claude/.credentials.json"),
            home.join(".claude.json"),
        ],
        HarnessProvider::Codex => vec![home.join(".codex/auth.json")],
        HarnessProvider::Opencode => vec![
            home.join(".local/share/opencode/auth.json"),
            home.join(".config/opencode/auth.json"),
        ],
    }
}

/// The user's home directory, or `None` when `HOME` is unset/empty.
fn home_dir() -> Option<PathBuf> {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .filter(|path| !path.as_os_str().is_empty())
}

/// Current unix time in seconds, or 0 if the clock is before the epoch.
fn now_unix_seconds() -> i64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}
