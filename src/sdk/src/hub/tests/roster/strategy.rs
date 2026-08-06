//! Which worker a route lands on: capacity strategies and subscription-budget
//! strategies, which pick from the same roster on different numbers.

use super::super::super::roster::{subscription_for_strategy, worker_for_strategy};
use super::helpers::{details, provider_budget, worker};
use crate::protocol::{AgentCapabilities, HarnessProvider, HarnessReadiness, WorkerSystemInfo};
use crate::runtime::{RoutingStrategy, SubscriptionRoutingStrategy};

#[test]
fn capacity_strategies_choose_different_workers() {
    let workers = vec![worker("cpu", "addr-cpu"), worker("ram", "addr-ram")];
    let details = std::collections::HashMap::from([
        ("cpu".into(), details(16, 4)),
        ("ram".into(), details(4, 64)),
    ]);
    assert_eq!(
        worker_for_strategy(&workers, &details, RoutingStrategy::CpuFirst).as_deref(),
        Some("cpu")
    );
    assert_eq!(
        worker_for_strategy(&workers, &details, RoutingStrategy::MemoryFirst).as_deref(),
        Some("ram")
    );
    assert_eq!(
        worker_for_strategy(&workers, &details, RoutingStrategy::Balanced).as_deref(),
        Some("cpu"),
        "balanced routing is CPU-first even when another worker has much more RAM"
    );
    assert_eq!(
        worker_for_strategy(&workers, &details, RoutingStrategy::Manual),
        None
    );
}

#[test]
fn balanced_uses_memory_to_break_cpu_ties_while_cpu_first_does_not() {
    let workers = vec![
        worker("larger", "addr-large"),
        worker("smaller", "addr-small"),
    ];
    let details = std::collections::HashMap::from([
        (
            "smaller".into(),
            WorkerSystemInfo {
                cpu_cores: 4,
                memory_total_bytes: None,
                memory_available_bytes: Some(128 * 1024 * 1024),
                ip_address: "10.0.0.1".into(),
            },
        ),
        (
            "larger".into(),
            WorkerSystemInfo {
                cpu_cores: 4,
                memory_total_bytes: None,
                memory_available_bytes: Some(896 * 1024 * 1024),
                ip_address: "10.0.0.2".into(),
            },
        ),
    ]);

    assert_eq!(
        worker_for_strategy(&workers, &details, RoutingStrategy::Balanced).as_deref(),
        Some("larger")
    );
    assert_eq!(
        worker_for_strategy(&workers, &details, RoutingStrategy::CpuFirst).as_deref(),
        Some("smaller"),
        "CPU First ignores RAM, so a CPU tie follows roster order"
    );
}

#[test]
fn subscription_strategies_compare_percentage_and_absolute_budget_independently() {
    let capabilities = AgentCapabilities {
        providers: vec![HarnessProvider::Claude, HarnessProvider::Codex],
        budgets: vec![
            provider_budget(HarnessProvider::Claude, 1_000, 800),
            provider_budget(HarnessProvider::Codex, 10_000, 2_000),
        ],
        ..Default::default()
    };

    assert_eq!(
        subscription_for_strategy(&capabilities, SubscriptionRoutingStrategy::Balanced),
        Some(HarnessProvider::Claude),
        "balanced compares normalized headroom"
    );
    assert_eq!(
        subscription_for_strategy(
            &capabilities,
            SubscriptionRoutingStrategy::MostAvailableBudget
        ),
        Some(HarnessProvider::Codex),
        "most-available compares absolute remaining tokens"
    );
    assert_eq!(
        subscription_for_strategy(&capabilities, SubscriptionRoutingStrategy::Manual),
        None,
        "manual preserves the task hint or daemon default"
    );
}

#[test]
fn subscription_routing_excludes_not_ready_and_fails_open_without_numbers() {
    let capabilities = AgentCapabilities {
        providers: vec![HarnessProvider::Claude, HarnessProvider::Codex],
        budgets: vec![
            provider_budget(HarnessProvider::Claude, 1_000, 900),
            provider_budget(HarnessProvider::Codex, 1_000, 100),
        ],
        readiness: vec![HarnessReadiness {
            provider: HarnessProvider::Claude,
            ready: false,
            reason: Some("cooldown".into()),
        }],
        ..Default::default()
    };

    assert_eq!(
        subscription_for_strategy(&capabilities, SubscriptionRoutingStrategy::Balanced),
        Some(HarnessProvider::Codex)
    );
    assert_eq!(
        subscription_for_strategy(
            &AgentCapabilities {
                providers: vec![HarnessProvider::Claude],
                ..Default::default()
            },
            SubscriptionRoutingStrategy::MostAvailableBudget
        ),
        None,
        "missing advisory budget data falls back to the daemon default"
    );
}
