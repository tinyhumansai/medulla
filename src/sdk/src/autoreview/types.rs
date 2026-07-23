//! Data types for composing and interpreting fresh-context reviews.

use std::path::PathBuf;

/// The implementation contract a reviewer must check.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReviewContract {
    /// Required outcome.
    pub outcome: String,
    /// Explicitly excluded work.
    pub non_goals: Vec<String>,
    /// Commands or checks that constitute verification.
    pub verify: Vec<String>,
}

/// Inputs needed to build one delegated review instruction.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReviewRequest {
    /// Original task being reviewed.
    pub task_id: String,
    /// Agent that implemented the task and must not review it.
    pub implementer_id: String,
    /// Different agent selected for the fresh review.
    pub reviewer_id: String,
    /// Repository whose changes are under review.
    pub workspace: PathBuf,
    /// Paths attributed to the implementation lane.
    pub touched_paths: Vec<PathBuf>,
    /// Parsed implementation contract.
    pub contract: ReviewContract,
    /// Exact patch for `touched_paths`.
    pub diff: String,
}

/// Structured terminal result recognized in review task notes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReviewVerdict {
    Approve,
    Findings(Vec<String>),
}

impl ReviewVerdict {
    /// Compact Agents-tab badge.
    pub fn badge(&self) -> String {
        match self {
            Self::Approve => "✓ reviewed".to_string(),
            Self::Findings(items) => format!("✗ findings({})", items.len()),
        }
    }
}

/// A review cannot be prepared without an independent reviewer or usable task.
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum ReviewError {
    #[error("review target '{0}' was not found")]
    UnknownTarget(String),
    #[error("review target '{0}' has no implementing agent")]
    MissingImplementer(String),
    #[error("no online agent other than '{0}' is available for independent review")]
    NoIndependentReviewer(String),
    #[error("review target '{0}' has no recorded instruction contract")]
    MissingContract(String),
    #[error("review target '{0}' has no local workspace")]
    MissingWorkspace(String),
    #[error("review diff failed: {0}")]
    Diff(String),
}
