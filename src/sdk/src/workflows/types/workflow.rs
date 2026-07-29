//! The stored workflow document and its listing and history views.
//!
//! These are the versioned half of the model: every write to a
//! [`WorkflowRecord`] snapshots the superseded copy as a [`WorkflowRevision`],
//! which is what makes an edit an operator disagrees with reversible.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use tinyflows::model::WorkflowGraph;

/// A workflow's stable identifier: the `id` in its document, defaulting to the
/// filename stem when the document omits one.
pub type WorkflowId = String;

/// A stored workflow: the engine graph plus where this host found it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct WorkflowRecord {
    /// The workflow's stable id.
    pub id: WorkflowId,
    /// Display name; falls back to the id when the document omits one.
    pub name: String,
    /// Operator-facing description of what the workflow does.
    #[serde(default)]
    pub description: String,
    /// Whether the workflow may be run. A disabled workflow still lists and
    /// validates, so an operator can repair one without it firing.
    #[serde(default = "default_enabled")]
    pub enabled: bool,
    /// The engine graph.
    pub graph: WorkflowGraph,
    /// The file this record was read from, when it came from disk. `None` for a
    /// graph built in memory (an agent's draft, an import not yet saved).
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub source_path: Option<PathBuf>,
}

/// Workflows are enabled unless a document says otherwise.
fn default_enabled() -> bool {
    true
}

impl WorkflowRecord {
    /// The listing view of this record.
    pub fn summary(&self) -> WorkflowSummary {
        WorkflowSummary {
            id: self.id.clone(),
            name: self.name.clone(),
            description: self.description.clone(),
            enabled: self.enabled,
            node_count: self.graph.nodes.len(),
            trigger_kind: self.trigger_kind(),
        }
    }

    /// The graph's trigger kind, as a lowercase string.
    ///
    /// Read out of the trigger node's free-form config rather than a typed
    /// field, because that is where the engine keeps it. `None` when the graph
    /// has no single trigger — which validation will also report, so this stays
    /// quiet rather than duplicating the error.
    pub fn trigger_kind(&self) -> Option<String> {
        let trigger = self.graph.trigger()?;
        trigger
            .config
            .get("trigger_kind")
            .and_then(|value| value.as_str())
            .map(str::to_string)
    }
}

/// A workflow reduced to what a list needs — the shape advertised to the
/// orchestrator and rendered in the TUI, so neither has to hold whole graphs.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkflowSummary {
    /// The workflow's stable id.
    pub id: WorkflowId,
    /// Display name.
    pub name: String,
    /// Operator-facing description.
    pub description: String,
    /// Whether the workflow may be run.
    pub enabled: bool,
    /// How many nodes the graph has.
    pub node_count: usize,
    /// The trigger kind, when the graph declares exactly one trigger.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub trigger_kind: Option<String>,
}

/// A copy of a workflow from before it was last written over.
///
/// Kept so an operator can disagree with an edit after the fact. That matters
/// most for the copilot, which writes to the store directly and would otherwise
/// leave a misread instruction as the only surviving version of a graph.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkflowRevision {
    /// This snapshot's id, unique within its workflow. Sorts chronologically.
    pub id: String,
    /// Epoch-millisecond stamp of when this copy stopped being current.
    ///
    /// When it was *superseded*, not when it was authored — a revision is
    /// named by the edit that replaced it, which is what an operator scanning
    /// history is looking for.
    pub superseded_at: u64,
    /// The workflow as it was.
    pub record: WorkflowRecord,
}
