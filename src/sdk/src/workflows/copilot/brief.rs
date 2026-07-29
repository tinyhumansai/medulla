//! What *this* turn is, as data rather than as four hand-assembled strings.
//!
//! The standing rules live in `prompt.md` and do not change between turns. What
//! changes is the situation: building from nothing, changing something that
//! exists, or diagnosing a run that failed. Modelling that as a
//! [`CopilotRequest`] rather than a pile of `format!` calls is what makes the
//! *modes* testable — a brief either names the failing run or it does not, and
//! that is now an assertion rather than a reading of prose.
//!
//! Adapted from the sibling `openhuman` host's `builder_prompt`, which arrived
//! at the same shape for the same reason. Its four modes are propose-only
//! because its copilot hands back a proposal card; Medulla's edits land in the
//! store and are taken back with undo instead, so the modes here differ in what
//! they *tell the agent to do* rather than in when they persist.

use tinyflows::model::WorkflowGraph;

use crate::workflows::WorkflowRecord;

/// How much of the current graph is pasted into the brief.
///
/// A large graph would crowd out the instruction and cost a round trip's worth
/// of tokens on something the agent can fetch itself. Past this it gets the
/// summary and is told to call `workflow_get` — which it needs to do anyway
/// before patching, so nothing is lost.
const MAX_INLINE_GRAPH_BYTES: usize = 6000;

/// Which kind of authoring turn this is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    /// Build a workflow that does not exist yet.
    Create,
    /// Change a workflow that does.
    Revise,
    /// Diagnose a run that failed and fix what caused it.
    ///
    /// The mode the copilot could not previously express. Without it an
    /// operator asking "why did this fail last night?" got a turn that knew
    /// only the graph — not the run, not the error, not which node stopped —
    /// and the agent had to go looking for all three before it could start.
    Repair,
}

// There is deliberately no `Explain` mode. A question is answered by a
// [`Mode::Revise`] turn that changes nothing, which the standing rules already
// require; a separate mode would mean classifying "is this a question?" at the
// call site, where nothing knows better than the model does.

impl Mode {
    /// The directive that leads the brief.
    ///
    /// Leading rather than trailing: it frames everything the agent reads
    /// after it, and a turn whose *kind* only became clear at the end would
    /// have been planned wrong by then.
    fn directive(self) -> &'static str {
        match self {
            Mode::Create => {
                "Build a new workflow that does what the operator describes below. \
                 Create exactly one, with `workflow_create`. Do not modify any workflow \
                 that already exists — this turn only adds."
            }
            Mode::Revise => {
                "Change the workflow named below, and only that one. Patch it with \
                 `workflow_apply_ops` rather than re-creating it."
            }
            Mode::Repair => {
                "A run of the workflow named below failed. Work out why and fix the \
                 cause. Read the run first — the failure is evidence about what the \
                 graph actually does, which is more reliable than what it looks like \
                 it does. If the cause is not something the graph can fix (a harness \
                 that was not installed, a host that refused the connection), say so \
                 and change nothing."
            }
        }
    }
}

/// One authoring turn, before it is rendered to a prompt.
#[derive(Debug, Clone)]
pub struct CopilotRequest<'a> {
    /// Which kind of turn this is.
    pub mode: Mode,
    /// The operator's own words, passed through unaltered.
    pub instruction: &'a str,
    /// The workflow being edited. `None` for [`Mode::Create`], which has none
    /// yet.
    pub record: Option<&'a WorkflowRecord>,
    /// The failed run this turn is about, for [`Mode::Repair`].
    pub run: Option<FailedRun>,
}

/// What is known about the run a [`Mode::Repair`] turn is fixing.
///
/// Carried into the brief rather than left for the agent to find, because it
/// already exists at the call site: the operator pressed a key next to a run
/// that is on screen. Making the agent spend a `workflow_runs` round trip to
/// rediscover what the caller knew is a turn it starts a step behind.
#[derive(Debug, Clone, Default)]
pub struct FailedRun {
    /// The run's id, so the agent can read the whole record.
    pub id: String,
    /// The failure message, when the run recorded one.
    pub error: Option<String>,
    /// Nodes implicated in the failure.
    pub failing_nodes: Vec<String>,
}

impl CopilotRequest<'_> {
    /// Render this turn as the text a harness is handed.
    ///
    /// Order is load-bearing: the directive frames the turn, the context
    /// grounds it, and the operator's words come last because a harness weights
    /// the end of a prompt most heavily — and their words are the part that
    /// changes.
    pub fn render(&self) -> String {
        let mut prompt = String::with_capacity(MAX_INLINE_GRAPH_BYTES + 2048);
        prompt.push_str(STANDING);
        prompt.push_str("\n\n# This turn\n\n");
        prompt.push_str(self.mode.directive());

        if let Some(record) = self.record {
            prompt.push_str("\n\n## The workflow\n\n");
            prompt.push_str(&format!("id: {}\nname: {}\n", record.id, record.name));
            if !record.description.trim().is_empty() {
                prompt.push_str(&format!("description: {}\n", record.description.trim()));
            }
            if !record.enabled {
                prompt.push_str(
                    "This workflow is disabled, so it will not run until an operator enables it.\n",
                );
            }
            prompt.push_str(&format!(
                "{} node{}, {} edge{}\n\n",
                record.graph.nodes.len(),
                if record.graph.nodes.len() == 1 {
                    ""
                } else {
                    "s"
                },
                record.graph.edges.len(),
                if record.graph.edges.len() == 1 {
                    ""
                } else {
                    "s"
                },
            ));
            prompt.push_str(&graph_section(&record.graph));
        }

        if let Some(run) = &self.run {
            prompt.push_str("\n\n## The failed run\n\n");
            prompt.push_str(&format!(
                "id: {}\nCall `workflow_runs` for the full record.\n",
                run.id
            ));
            if let Some(error) = run
                .error
                .as_deref()
                .map(str::trim)
                .filter(|e| !e.is_empty())
            {
                prompt.push_str(&format!("\nIt failed with:\n\n```\n{error}\n```\n"));
            }
            if !run.failing_nodes.is_empty() {
                prompt.push_str(&format!(
                    "\nNodes implicated: {}\n",
                    run.failing_nodes.join(", ")
                ));
            }
        }

        prompt.push_str("\n\n## The instruction\n\n");
        prompt.push_str(self.instruction.trim());
        prompt.push('\n');
        prompt
    }
}

/// The rules that do not change between turns.
///
/// Kept as prose in a sibling file rather than a string constant: it is long,
/// it is edited far more often than the code around it, and a change to it
/// should read as a change to a document in review rather than as a diff
/// through escaped quotes.
const STANDING: &str = include_str!("prompt.md");

/// The graph itself, inline when it is small enough to be worth pasting.
fn graph_section(graph: &WorkflowGraph) -> String {
    let json = serde_json::to_string_pretty(graph).unwrap_or_else(|_| "{}".to_string());
    if json.len() <= MAX_INLINE_GRAPH_BYTES {
        return format!("Current graph:\n\n```json\n{json}\n```");
    }
    // Too big to paste: the outline is enough to plan against, and the agent
    // fetches the detail for the nodes it actually touches.
    let outline: Vec<String> = graph
        .nodes
        .iter()
        .map(|node| {
            format!(
                "- {} ({}){}",
                node.id,
                crate::ui::workflows::graph::kind_wire(&node.kind),
                if node.name.trim().is_empty() {
                    String::new()
                } else {
                    format!(" — {}", node.name.trim())
                }
            )
        })
        .collect();
    format!(
        "The graph is too large to paste. Call `workflow_get` for it. Its nodes:\n\n{}",
        outline.join("\n")
    )
}

#[cfg(test)]
#[path = "brief_tests.rs"]
mod tests;
