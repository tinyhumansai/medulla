//! A host budget is checked before dispatch and after each charged model call.

use openhuman_embed::seams::{StopDecision, StopHook, TurnState};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

pub(super) struct TokenBudget {
    limit: u64,
    observed: AtomicU64,
    host: Arc<super::EmbedHost>,
}
impl TokenBudget {
    pub fn new(limit: u64, host: Arc<super::EmbedHost>) -> Self {
        Self {
            limit,
            observed: AtomicU64::new(0),
            host,
        }
    }
}
#[async_trait::async_trait]
impl StopHook for TokenBudget {
    fn name(&self) -> &str {
        "medulla.token-budget"
    }
    async fn check(&self, state: &TurnState<'_>) -> StopDecision {
        let tokens = state
            .cost
            .input_tokens
            .saturating_add(state.cost.output_tokens);
        let before = self.observed.fetch_max(tokens, Ordering::SeqCst);
        self.host.charge(tokens.saturating_sub(before));
        if tokens >= self.limit || self.host.token_headroom() == Some(0) {
            StopDecision::Stop {
                reason: "Medulla worker token budget exhausted".into(),
            }
        } else {
            StopDecision::Continue
        }
    }
}
