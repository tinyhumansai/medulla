//! What a selected node says about itself, and how a run marks up the graph.
//!
//! Two views over one node. [`node_detail`] is the static one — the config an
//! author wrote, spelled out as rows. [`RunOverlay`] is the live one: given a
//! run record, it says what each node's box should show, so the same layout that
//! draws a plan also draws the last attempt at it.
//!
//! Keeping the overlay separate from the layout is deliberate. The plan does not
//! change between runs, and re-laying it out for every status refresh would move
//! the boxes under the operator's cursor.

use std::collections::HashMap;

use tinyflows::model::{Node, WorkflowGraph};

use crate::workflows::{RunRecord, RunStatus};

/// One labelled line of a node's declaration.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DetailRow {
    /// The field name, or empty for a continuation line of the value above.
    pub label: String,
    /// The value, already flattened to one line.
    pub value: String,
}

/// How a run touched one node.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NodeRunState {
    /// The run has not reached this node.
    Pending,
    /// The node ran and succeeded.
    Succeeded,
    /// The node ran and failed.
    Failed,
    /// The node is parked on an approval gate.
    AwaitingApproval,
    /// The node ran and reported something else — a status the engine grew that
    /// this host has no opinion about yet, shown as-is rather than guessed at.
    Other,
}

impl NodeRunState {
    /// The operator-facing word.
    pub fn label(self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Succeeded => "ok",
            Self::Failed => "failed",
            Self::AwaitingApproval => "awaiting approval",
            Self::Other => "ran",
        }
    }

    /// A colour name in the vocabulary the app crate maps to a theme.
    pub fn color(self) -> &'static str {
        match self {
            Self::Pending => "gray",
            Self::Succeeded => "green",
            Self::Failed => "red",
            Self::AwaitingApproval => "yellow",
            Self::Other => "blue",
        }
    }

    /// A one-character marker, for the corner of a node's box.
    pub fn glyph(self) -> &'static str {
        match self {
            Self::Pending => "·",
            Self::Succeeded => "✓",
            Self::Failed => "✗",
            Self::AwaitingApproval => "⏸",
            Self::Other => "•",
        }
    }
}

/// What one node's box shows while a run is selected.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NodeRun {
    /// How the run left this node.
    pub state: NodeRunState,
    /// Wall-clock duration, when the node ran.
    pub duration_ms: Option<u128>,
    /// Expressions that resolved to null — usually a wiring mistake, and the
    /// thing an operator opens a finished run to find.
    pub diagnostics: Vec<String>,
}

/// A run, indexed by node so the layout can be marked up in one pass.
#[derive(Debug, Clone, Default)]
pub struct RunOverlay {
    by_node: HashMap<String, NodeRun>,
}

impl RunOverlay {
    /// Index `record`'s steps and pending gates by node id.
    ///
    /// A node appearing twice — a loop, a retry — keeps its *last* step, which
    /// is the state the run actually ended in.
    pub fn new(record: &RunRecord) -> Self {
        let mut by_node: HashMap<String, NodeRun> = HashMap::new();
        for step in &record.steps {
            by_node.insert(
                step.node_id.clone(),
                NodeRun {
                    state: state_for(&step.status),
                    duration_ms: Some(step.duration_ms),
                    diagnostics: step.diagnostics.clone(),
                },
            );
        }
        // A gate the run is parked on outranks whatever step record the node
        // has: it is where the run *is*, not where it has been.
        for node_id in &record.pending_approvals {
            let entry = by_node.entry(node_id.clone()).or_insert(NodeRun {
                state: NodeRunState::AwaitingApproval,
                duration_ms: None,
                diagnostics: Vec::new(),
            });
            entry.state = NodeRunState::AwaitingApproval;
        }
        Self { by_node }
    }

    /// How the run left `node_id`. Never absent: a node the run did not reach is
    /// [`NodeRunState::Pending`], which is a fact about the run rather than a
    /// gap in it.
    pub fn node(&self, node_id: &str) -> NodeRun {
        self.by_node.get(node_id).cloned().unwrap_or(NodeRun {
            state: NodeRunState::Pending,
            duration_ms: None,
            diagnostics: Vec::new(),
        })
    }

    /// How many nodes the run got through.
    pub fn reached(&self) -> usize {
        self.by_node.len()
    }
}

/// Map an engine step status onto a state this host draws.
fn state_for(status: &str) -> NodeRunState {
    match status.trim().to_ascii_lowercase().as_str() {
        "success" | "succeeded" | "ok" | "completed" => NodeRunState::Succeeded,
        "failed" | "error" => NodeRunState::Failed,
        "pending_approval" | "awaiting_approval" | "paused" => NodeRunState::AwaitingApproval,
        _ => NodeRunState::Other,
    }
}

/// Whether a run status means the graph should be drawn as live.
pub fn is_live(status: RunStatus) -> bool {
    matches!(status, RunStatus::Running | RunStatus::PendingApproval)
}

/// The selected node's declaration, as labelled rows.
///
/// The config is free-form JSON per kind, so it is flattened generically rather
/// than per kind: every scalar becomes a row under its dotted path, and nested
/// objects and arrays are walked. Long values are left whole — the renderer
/// clips to the pane, and truncating here would hide the end of a URL from a
/// wide terminal that had room for it.
pub fn node_detail(node: &Node) -> Vec<DetailRow> {
    let mut rows = vec![
        DetailRow {
            label: "id".into(),
            value: node.id.clone(),
        },
        DetailRow {
            label: "kind".into(),
            value: super::graph::kind_wire(&node.kind).to_string(),
        },
    ];
    if !node.name.trim().is_empty() {
        rows.push(DetailRow {
            label: "name".into(),
            value: node.name.clone(),
        });
    }
    if node.type_version != 1 {
        rows.push(DetailRow {
            label: "type_version".into(),
            value: node.type_version.to_string(),
        });
    }
    for port in &node.ports {
        rows.push(DetailRow {
            label: "port".into(),
            value: match &port.label {
                Some(label) => format!("{} — {label}", port.name),
                None => port.name.clone(),
            },
        });
    }
    flatten(&node.config, "", &mut rows);
    rows
}

/// Walk a config value into `label: value` rows under dotted paths.
fn flatten(value: &serde_json::Value, prefix: &str, out: &mut Vec<DetailRow>) {
    let path = |key: &str| {
        if prefix.is_empty() {
            key.to_string()
        } else {
            format!("{prefix}.{key}")
        }
    };
    match value {
        serde_json::Value::Object(map) => {
            for (key, child) in map {
                flatten(child, &path(key), out);
            }
        }
        serde_json::Value::Array(items) => {
            // An empty array still says something — a merge with no inputs is a
            // bug — so it gets a row rather than vanishing.
            if items.is_empty() {
                out.push(DetailRow {
                    label: prefix.to_string(),
                    value: "[]".into(),
                });
            }
            for (index, child) in items.iter().enumerate() {
                flatten(child, &path(&index.to_string()), out);
            }
        }
        serde_json::Value::Null => out.push(DetailRow {
            label: prefix.to_string(),
            value: "null".into(),
        }),
        other => {
            let text = match other {
                serde_json::Value::String(text) => text.clone(),
                other => other.to_string(),
            };
            // A multi-line value (a prompt, a code body) becomes one labelled
            // row plus unlabelled continuations, so the label column stays a
            // column.
            let mut lines = text.split('\n');
            out.push(DetailRow {
                label: prefix.to_string(),
                value: lines.next().unwrap_or_default().to_string(),
            });
            for line in lines {
                out.push(DetailRow {
                    label: String::new(),
                    value: line.to_string(),
                });
            }
        }
    }
}

/// Find a node in `graph` by id.
pub fn find_node<'a>(graph: &'a WorkflowGraph, id: &str) -> Option<&'a Node> {
    graph.nodes.iter().find(|node| node.id == id)
}

#[cfg(test)]
#[path = "inspect_tests.rs"]
mod tests;
