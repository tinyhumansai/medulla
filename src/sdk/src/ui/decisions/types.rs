//! Data shapes for the prepared-decision queue.

/// Why an item needs operator attention.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DecisionKind {
    /// A harness-level escalation without a structured answer target.
    Escalation,
    /// A worker question that can route through `question.answer`.
    WorkerQuestion,
}

/// Wire ids required to answer one worker question.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DecisionAnswerTarget {
    /// Cycle that owns the pending question.
    pub cycle_id: String,
    /// Stable question id accepted by the harness.
    pub question_id: String,
}

/// One deduplicated, operator-ready decision.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DecisionItem {
    /// Stable local id used for dismissal and selection.
    pub id: String,
    /// Item classification.
    pub kind: DecisionKind,
    /// Prepared question or escalation message.
    pub question: String,
    /// Lane/task context shown beside the question.
    pub lane_context: String,
    /// Best available contract/task excerpt.
    pub contract_excerpt: Option<String>,
    /// Answer routing metadata; absent for informational escalations.
    pub answer_target: Option<DecisionAnswerTarget>,
}
