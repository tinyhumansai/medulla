//! Prepared operator decisions derived from harness escalations and pending
//! worker questions. The fold is UI-agnostic so terminal and future hosts share
//! stable ids, ordering, deduplication, and answer routing.

mod fold;
mod types;

#[cfg(test)]
mod tests;

pub use fold::decision_items;
pub use types::{DecisionAnswerTarget, DecisionItem, DecisionKind};
