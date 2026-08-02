//! Data types and ordered choices for routing-strategy selectors.

use medulla::runtime::{RoutingStrategy, SubscriptionRoutingStrategy};

/// Display metadata coupled to the routing strategy it applies.
#[derive(Clone, Copy)]
pub(in crate::ui::app) struct RoutingStrategyOption {
    /// Runtime strategy sent when the option is applied.
    pub(in crate::ui::app) strategy: RoutingStrategy,
    /// Short label rendered in the strategy chooser.
    pub(in crate::ui::app) label: &'static str,
    /// Operator-facing explanation of the selection rule.
    pub(in crate::ui::app) description: &'static str,
}

/// Routing strategy options in the order shown by the chooser.
pub(in crate::ui::app) const ROUTING_STRATEGIES: [RoutingStrategyOption; 4] = [
    RoutingStrategyOption {
        strategy: RoutingStrategy::Manual,
        label: "Manual",
        description: "Keep the host explicitly selected on the Hosts page.",
    },
    RoutingStrategyOption {
        strategy: RoutingStrategy::Balanced,
        label: "Balanced",
        description: "Choose the most CPU cores, breaking ties by available RAM.",
    },
    RoutingStrategyOption {
        strategy: RoutingStrategy::CpuFirst,
        label: "CPU First",
        description: "Choose the host with the most logical CPU cores.",
    },
    RoutingStrategyOption {
        strategy: RoutingStrategy::MemoryFirst,
        label: "Memory First",
        description: "Choose the host with the most currently available RAM.",
    },
];

/// Display metadata for one subscription-level selection rule.
#[derive(Clone, Copy)]
pub(in crate::ui::app) struct SubscriptionStrategyOption {
    /// Runtime strategy sent when the option is applied.
    pub(in crate::ui::app) strategy: SubscriptionRoutingStrategy,
    /// Short label rendered in the strategy chooser.
    pub(in crate::ui::app) label: &'static str,
    /// Operator-facing explanation of the budget comparison.
    pub(in crate::ui::app) description: &'static str,
}

/// Subscription strategy options in the order shown by the chooser.
pub(in crate::ui::app) const SUBSCRIPTION_STRATEGIES: [SubscriptionStrategyOption; 3] = [
    SubscriptionStrategyOption {
        strategy: SubscriptionRoutingStrategy::Manual,
        label: "Manual",
        description: "Keep the requested provider or the host's configured default.",
    },
    SubscriptionStrategyOption {
        strategy: SubscriptionRoutingStrategy::Balanced,
        label: "Balanced",
        description: "Choose the ready subscription with the most remaining percentage.",
    },
    SubscriptionStrategyOption {
        strategy: SubscriptionRoutingStrategy::MostAvailableBudget,
        label: "Most Available Budget",
        description: "Choose the ready subscription with the most remaining tokens.",
    },
];
