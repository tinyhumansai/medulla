//! The run protocol and data types: what a granted process reports, and what
//! the registry accumulates from those reports.
//!
//! Split from [`super::registry`] so the shape the wire and the rail agree on
//! reads without the retention rules around it.

use serde::{Deserialize, Serialize};

/// How the reporting side says a run is going.
///
/// Deliberately not [`crate::workflows::RunStatus`]: this module sits below the
/// `workflows` feature and must compile without it, and the wire word is what
/// the rail renders anyway.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum HarnessRunStatus {
    /// The run is executing.
    Running,
    /// It finished, and every step succeeded.
    Succeeded,
    /// It failed, or ended without settling.
    Failed,
    /// It is parked on an approval gate.
    AwaitingApproval,
}

impl HarnessRunStatus {
    /// Read a wire word, defaulting to [`Running`](Self::Running).
    ///
    /// An unknown word reads as running rather than as failed: a status this
    /// build does not know is a peer that grew one, and reporting an executing
    /// run as failed is the more misleading of the two guesses.
    pub fn from_wire(value: &str) -> Self {
        match value.trim().to_ascii_lowercase().as_str() {
            "succeeded" | "success" | "ok" => Self::Succeeded,
            "failed" | "error" | "interrupted" | "cancelled" => Self::Failed,
            "awaiting_approval" | "awaitingapproval" | "pending_approval" => Self::AwaitingApproval,
            _ => Self::Running,
        }
    }

    /// The operator-facing word.
    pub fn label(self) -> &'static str {
        match self {
            Self::Running => "running",
            Self::Succeeded => "ok",
            Self::Failed => "failed",
            Self::AwaitingApproval => "awaiting approval",
        }
    }

    /// A colour name in the vocabulary the app crate maps to a theme.
    pub fn color(self) -> &'static str {
        match self {
            Self::Running => "yellow",
            Self::Succeeded => "green",
            Self::Failed => "red",
            Self::AwaitingApproval => "cyan",
        }
    }

    /// Whether the run has settled.
    pub fn is_terminal(self) -> bool {
        matches!(self, Self::Succeeded | Self::Failed)
    }
}

/// One progress frame a run's harness emitted.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HarnessRunFrame {
    /// The graph node whose harness emitted it, when the reporter said.
    pub node: Option<String>,
    /// The frame itself, in the vocabulary
    /// [`crate::daemon::status_detail`] writes.
    pub text: String,
}

/// One workflow run a granted session started.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HarnessRun {
    /// The engine run id, which the Workflows page can select by.
    pub run_id: String,
    /// The workflow being run.
    pub workflow_id: String,
    /// Where it is.
    pub status: HarnessRunStatus,
    /// Epoch ms of the first report.
    pub started_at: i64,
    /// Epoch ms of the most recent report.
    pub updated_at: i64,
    /// The latest thing the run said about itself — a step, or a line the
    /// harness under it emitted. `None` before anything happened.
    pub detail: Option<String>,
    /// The recent frames its harnesses emitted, oldest first.
    ///
    /// Kept as well as `detail` because the two answer different questions: the
    /// rail row shows the newest line in one row, and the Workflows page draws
    /// the tail under the node that produced it.
    pub frames: Vec<HarnessRunFrame>,
}

/// One report from a granted process about a run it is executing.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RunReport {
    /// The engine run id.
    pub run_id: String,
    /// The workflow being run.
    pub workflow_id: String,
    /// Where the run is now.
    pub status: HarnessRunStatus,
    /// What just happened, when there is something worth saying.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
    /// The graph node the detail came from, for a frame streamed by a step's
    /// harness. Absent on a lifecycle report, which is about the run itself.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub node: Option<String>,
}
