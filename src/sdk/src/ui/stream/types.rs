//! Plain data types produced by the stream derivations in
//! [`super`](crate::ui::stream) — token-usage totals folded from the event log.

use std::collections::BTreeMap;

/// Token totals for one inference tier (orchestrator / reasoning / compress /
/// …), as shown on the Usage tab.
#[derive(Debug, Default, Clone, PartialEq)]
pub struct TierUsage {
    /// Prompt (input) tokens billed to the tier.
    pub input_tokens: i64,
    /// Completion (output) tokens billed to the tier.
    pub output_tokens: i64,
    /// Number of inference calls started on the tier.
    pub calls: i64,
}

/// The Usage tab's fold over the live event stream: per-tier and per-task token
/// accounting derived purely from the event log.
#[derive(Debug, Default, Clone, PartialEq)]
pub struct UsageFold {
    /// Per-tier totals, keyed by tier name and ordered for stable display.
    pub tiers: BTreeMap<String, TierUsage>,
    /// Aggregate sub-agent (delegated task) usage across every completed task.
    pub subagent: TierUsage,
    /// Per-task `(task id, input tokens, output tokens)` rows, in arrival order.
    pub tasks: Vec<(String, i64, i64)>,
}

impl UsageFold {
    /// Mutable access to a tier's totals, inserting a zeroed entry on first use.
    pub(super) fn tier_mut(&mut self, tier: &str) -> &mut TierUsage {
        self.tiers.entry(tier.to_string()).or_default()
    }
}
