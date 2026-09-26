//! What an evolution pass is asked to do, and what it produces.

use crate::workflows::{RunId, WorkflowNote, WorkflowProposal};

/// Why a pass is running.
///
/// Carried into the brief rather than inferred: a pass that starts because a
/// run just failed should lead with that run, and one an operator asked for
/// should lead with their question.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EvolveTrigger {
    /// A run ended in failure.
    Failure(RunId),
    /// An operator asked for a review.
    Manual,
}

impl EvolveTrigger {
    /// The run this pass is about, when it is about one.
    pub fn run_id(&self) -> Option<&str> {
        match self {
            Self::Failure(id) => Some(id.as_str()),
            Self::Manual => None,
        }
    }
}

/// What a pass left behind.
///
/// Read back off the store rather than taken from the agent's reply, on the
/// same principle as [`crate::workflows::copilot::diff`]: what a turn *did* is
/// what is on disk, not what it said it did.
#[derive(Debug, Clone, Default)]
pub struct EvolveOutcome {
    /// The agent's own words, for the pane.
    pub reply: String,
    /// Every note this pass durably wrote — the agent's, plus the system note
    /// that is written whether or not the agent wrote anything.
    pub notes: Vec<WorkflowNote>,
    /// Proposals this pass produced, each already verified.
    pub proposals: Vec<WorkflowProposal>,
    /// Set when the pass did not run because one was already in flight.
    ///
    /// Not an error: a workflow failing in a burst is the normal case, and the
    /// second failure has nothing new to say that the first pass will not
    /// already find.
    pub skipped: bool,
}

/// How much history a pass reads, and whether it runs at all.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EvolveConfig {
    /// Whether passes may run on this host.
    pub enabled: bool,
    /// Whether a failed run starts one by itself.
    pub auto_on_failure: bool,
    /// How many recent runs reach the brief.
    pub max_runs: usize,
    /// How many current notes reach the brief.
    pub max_notes: usize,
}

impl EvolveConfig {
    /// Read this host's settings.
    pub fn from_config(config: &crate::config::WorkflowsConfig) -> Self {
        Self {
            // A host with workflows switched off has no runs to review, so the
            // outer switch subsumes the inner one.
            enabled: config.enabled && config.evolve.enabled,
            auto_on_failure: config.evolve.auto_on_failure,
            max_runs: config.evolve.max_runs,
            max_notes: config.evolve.max_notes,
        }
    }
}

impl Default for EvolveConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            auto_on_failure: true,
            // Five is enough to see a pattern and few enough that the brief is
            // still mostly the graph and the notes. The cap bounds what the
            // agent reads, not what the store scans.
            max_runs: 5,
            max_notes: 40,
        }
    }
}
