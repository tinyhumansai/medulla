//! Typed pull-request and CI state returned by the ship client.

use std::path::PathBuf;

/// Aggregate state of the checks attached to a pull request.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CheckState {
    /// Every reported check completed successfully.
    Green,
    /// At least one check failed or was cancelled.
    Failing,
    /// At least one check is queued or running, or no checks were reported.
    Pending,
}

impl CheckState {
    /// Compact label used by text and TUI clients.
    pub fn label(self) -> &'static str {
        match self {
            Self::Green => "green",
            Self::Failing => "failing",
            Self::Pending => "pending",
        }
    }
}

/// Observable pull-request state for one open PR.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PrSummary {
    /// Repository-local pull-request number.
    pub number: u64,
    /// Pull-request title.
    pub title: String,
    /// Head branch name.
    pub head: String,
    /// Browser URL reported by GitHub.
    pub url: String,
    /// Aggregate check state.
    pub checks: CheckState,
    /// Number of review threads not marked resolved.
    pub unresolved_threads: usize,
}

/// Best-effort ship state for one configured workspace.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkspaceShipReport {
    /// Inspected local repository root.
    pub root: PathBuf,
    /// Available PR rows, or an explicit reason the `gh` surface is unavailable.
    pub state: ShipState,
}

/// The ship surface never crashes when GitHub CLI access is unavailable.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ShipState {
    /// GitHub CLI was authenticated and returned the open PR list.
    Ready(Vec<PrSummary>),
    /// GitHub CLI was missing, unauthenticated, or failed its read probe.
    GhUnavailable(String),
}

/// Typed failures for explicit ship actions.
#[derive(Debug, thiserror::Error)]
pub enum ShipError {
    /// The configured `gh` executable could not be started.
    #[error("gh unavailable: {0}")]
    Unavailable(String),
    /// A `gh` command returned a non-zero status.
    #[error("gh command failed: {0}")]
    Command(String),
    /// GitHub CLI returned malformed JSON.
    #[error("gh returned malformed JSON: {0}")]
    Decode(#[from] serde_json::Error),
    /// The local repository has no parseable upstream remote.
    #[error("no canonical upstream remote is configured")]
    MissingUpstream,
}
