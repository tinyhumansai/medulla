//! Workflow runs a granted harness started, reported back to the Medulla that
//! granted it.
//!
//! A harness reaching `workflow_run` over MCP executes the run *in the MCP
//! subprocess*: the engine, the embedded daemon, and every harness session it
//! dispatches live there, not in the Medulla the operator is looking at. So
//! from the operator's side the most consequential thing a session can do — set
//! a whole workflow going — was invisible until it finished and a run record
//! appeared on disk.
//!
//! This is the channel that closes that gap. The subprocess reports what it
//! started, how the run is progressing, and how it ended; the reports are keyed
//! by the *grant's session*, which is the same key the launcher recorded on the
//! PTY row ([`LaunchSpec::mcp_grant_session`](crate::mcp::attach)). That is what
//! lets the rail draw the run underneath the session that triggered it rather
//! than in a list of runs with no context.
//!
//! Everything here is bounded and best-effort. A report that cannot be
//! delivered never fails the run: the run is the work, and this is the view of
//! it.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use serde::{Deserialize, Serialize};

/// The most runs retained per session.
///
/// A session that starts run after run keeps its recent ones; the rest is the
/// workflow's own durable history, which the Workflows page reads from disk.
pub(super) const MAX_RUNS_PER_SESSION: usize = 8;

/// The most progress frames retained per run.
///
/// Enough to watch a step work; the run's own record is where the durable
/// account of it lives.
pub(super) const MAX_FRAMES_PER_RUN: usize = 200;

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

/// The runs every granted session has reported, keyed by grant session.
///
/// Cheap to clone; every clone shares one table.
#[derive(Clone, Default)]
pub struct HarnessRunRegistry {
    inner: Arc<Mutex<HashMap<String, Vec<HarnessRun>>>>,
}

impl HarnessRunRegistry {
    /// An empty registry.
    pub fn new() -> Self {
        Self::default()
    }

    /// Record `report` against `session`, creating the run on first sight.
    ///
    /// A repeat report updates the run in place, so a run that emits a hundred
    /// progress lines stays one row rather than a hundred.
    pub fn report(&self, session: &str, report: RunReport) {
        let now = crate::clock::now_millis();
        let Ok(mut runs) = self.inner.lock() else {
            return;
        };
        let entry = runs.entry(session.to_string()).or_default();
        if let Some(existing) = entry.iter_mut().find(|run| run.run_id == report.run_id) {
            existing.status = report.status;
            existing.updated_at = now;
            // A report with nothing to say still moves the clock; it must not
            // blank out the last thing that did.
            if let Some(detail) = report.detail {
                push_frame(existing, report.node, detail);
            }
            return;
        }
        let mut run = HarnessRun {
            run_id: report.run_id,
            workflow_id: report.workflow_id,
            status: report.status,
            started_at: now,
            updated_at: now,
            detail: None,
            frames: Vec::new(),
        };
        if let Some(detail) = report.detail {
            push_frame(&mut run, report.node, detail);
        }
        entry.push(run);
        // Oldest settled first: a run still executing is the one the operator
        // is watching, and dropping it to keep a finished one would be exactly
        // backwards.
        while entry.len() > MAX_RUNS_PER_SESSION {
            let victim = entry
                .iter()
                .position(|run| run.status.is_terminal())
                .unwrap_or(0);
            entry.remove(victim);
        }
    }

    /// The run with this id, wherever it was reported from.
    ///
    /// Runs are keyed by session because that is how they are *drawn*; a reader
    /// that already has a run id — the Workflows page, following a jump from
    /// the rail — has no session to look it up under.
    pub fn find(&self, run_id: &str) -> Option<HarnessRun> {
        self.inner
            .lock()
            .ok()?
            .values()
            .flatten()
            .find(|run| run.run_id == run_id)
            .cloned()
    }

    /// What `session` has running and recently ran, oldest first.
    pub fn for_session(&self, session: &str) -> Vec<HarnessRun> {
        self.inner
            .lock()
            .ok()
            .and_then(|runs| runs.get(session).cloned())
            .unwrap_or_default()
    }

    /// Forget everything `session` reported.
    ///
    /// Called when the session's grant is revoked: the rows belong to a harness
    /// that is gone, and leaving them under a row that no longer exists would
    /// strand them at the bottom of the rail.
    pub fn forget(&self, session: &str) {
        if let Ok(mut runs) = self.inner.lock() {
            runs.remove(session);
        }
    }
}

/// Record one frame on `run`, keeping the newest and dropping past the bound.
fn push_frame(run: &mut HarnessRun, node: Option<String>, text: String) {
    run.detail = Some(text.clone());
    run.frames.push(HarnessRunFrame { node, text });
    if run.frames.len() > MAX_FRAMES_PER_RUN {
        let excess = run.frames.len() - MAX_FRAMES_PER_RUN;
        run.frames.drain(..excess);
    }
}
