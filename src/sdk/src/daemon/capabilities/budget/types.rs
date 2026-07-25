//! Data types for the budget/readiness probe: the operator-declared numbers,
//! the pure core's per-provider input, and the injectable environment seams.
//! Behaviour-heavy `impl`s (seam wiring, evaluation) live beside the logic in
//! [`super`].

use crate::tinyplace::{BudgetWindow, HarnessProvider};

/// Operator-declared budget numbers for one provider, read through a seam.
///
/// Every field is optional; a caller sets only what the operator specified. When
/// present, it makes the emitted [`crate::tinyplace::HarnessBudget`] authoritative
/// ([`crate::tinyplace::BudgetSource::Configured`]) rather than an estimate.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ConfiguredBudget {
    /// Opaque seat identifier the operator recorded (never a credential).
    pub seat: Option<String>,
    /// The metering window the allowance renews on.
    pub window: Option<BudgetWindow>,
    /// The configured allowance for the window.
    pub limit_tokens: Option<i64>,
    /// Consumption recorded so far in the window.
    pub used_tokens: Option<i64>,
    /// Remaining allowance, when the operator records it directly. Preferred over
    /// the `limit - used` derivation when set.
    pub remaining_tokens: Option<i64>,
    /// Unix seconds until which the seat is parked.
    pub cooldown_until: Option<i64>,
}

impl ConfiguredBudget {
    /// Build from the operator's `[budget.providers.<p>]` config for one provider.
    ///
    /// A pure field-for-field copy: the config type mirrors this one exactly, so
    /// no credential or derived value is invented here.
    pub fn from_config(cfg: &crate::config::ProviderBudgetConfig) -> Self {
        ConfiguredBudget {
            seat: cfg.seat.clone(),
            window: cfg.window,
            limit_tokens: cfg.limit_tokens,
            used_tokens: cfg.used_tokens,
            remaining_tokens: cfg.remaining_tokens,
            cooldown_until: cfg.cooldown_until,
        }
    }
}

/// The already-gathered facts for one provider — the pure core's whole input.
///
/// Producing this from the real environment is [`BudgetSeams`]'s job; the core
/// takes it as plain data so it can be exhaustively unit-tested.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProviderProbeInput {
    /// The provider being evaluated.
    pub provider: HarnessProvider,
    /// Whether the provider's binary is present on the machine.
    pub installed: bool,
    /// Whether the harness appears authenticated: `Some(true)` when a positive
    /// signal was found, `Some(false)` only on a definitive logged-out signal,
    /// and `None` when it could not be determined (treated as usable — fail open).
    pub authenticated: Option<bool>,
    /// A detected throttle/park expiry in unix seconds, if any.
    pub cooldown_until: Option<i64>,
    /// Operator-configured numbers, when the config seam supplied them.
    pub configured: Option<ConfiguredBudget>,
    /// Current unix time, injected so cooldown comparisons stay pure.
    pub now_unix: i64,
}

/// Injectable environment seams the collector depends on.
///
/// The default wiring ([`BudgetSeams::from_env`]) reads the real filesystem and
/// process environment; tests supply closures over fixed data so the probe stays
/// deterministic and offline.
pub struct BudgetSeams {
    /// Whether a provider's binary resolves on the machine.
    pub installed: Box<dyn Fn(HarnessProvider) -> bool + Send + Sync>,
    /// The provider's authentication signal (see [`ProviderProbeInput::authenticated`]).
    pub authenticated: Box<dyn Fn(HarnessProvider) -> Option<bool> + Send + Sync>,
    /// A detected cooldown expiry for the provider, if any.
    pub cooldown_until: Box<dyn Fn(HarnessProvider) -> Option<i64> + Send + Sync>,
    /// Operator-configured numbers for the provider, if any.
    pub configured: Box<dyn Fn(HarnessProvider) -> Option<ConfiguredBudget> + Send + Sync>,
    /// The current unix time, injected for pure cooldown comparisons.
    pub now_unix: Box<dyn Fn() -> i64 + Send + Sync>,
}
