//! The sections of a brief that describe the *situation* rather than the task.
//!
//! Everything here answers "what is the agent looking at": the graph as it
//! stands today, and — as the host learns more about a workflow — whatever else
//! grounds the turn. These grow independently of the turn structure in the
//! parent module, which is why they are apart from it.

use tinyflows::model::WorkflowGraph;

use crate::workflows::{NoteSource, RunRecord, RunStatus, WorkflowNote};

/// How much of the current graph is pasted into the brief.
///
/// A large graph would crowd out the instruction and cost a round trip's worth
/// of tokens on something the agent can fetch itself. Past this it gets the
/// summary and is told to call `workflow_get` — which it needs to do anyway
/// before patching, so nothing is lost.
pub(super) const MAX_INLINE_GRAPH_BYTES: usize = 6000;

/// The graph itself, inline when it is small enough to be worth pasting.
pub(super) fn graph_section(graph: &WorkflowGraph) -> String {
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

/// What the host has already learned about this workflow.
///
/// Attribution is spelled out per note rather than left implicit: an operator's
/// constraint and a model's own hypothesis are not the same kind of claim, and
/// a list that presented them identically would let a previous turn's guess
/// outweigh what a person actually said.
pub(super) fn notes_section(notes: &[WorkflowNote]) -> String {
    if notes.is_empty() {
        return "Nothing has been recorded about this workflow yet. Whatever you conclude \
                in this turn is the first thing it will know next time."
            .to_string();
    }
    let lines: Vec<String> = notes
        .iter()
        .map(|note| {
            let who = match &note.source {
                NoteSource::Operator => "operator".to_string(),
                NoteSource::System => "host".to_string(),
                NoteSource::Agent { model: Some(model) } => format!("agent/{model}"),
                NoteSource::Agent { model: None } => "agent".to_string(),
            };
            let kind = format!("{:?}", note.kind).to_lowercase();
            let evidence = if note.run_ids.is_empty() {
                String::new()
            } else {
                format!(" [from {}]", note.run_ids.join(", "))
            };
            format!("- ({kind}, {who}) {}{evidence}", note.text.trim())
        })
        .collect();
    lines.join("\n")
}

/// The recent runs, as one line each plus the evidence a failure left.
///
/// Deliberately not the whole records: the agent can fetch one with
/// `workflow_run_get` when it wants the detail, and pasting five in full would
/// crowd out the graph and the notes for information it mostly will not read.
pub(super) fn runs_section(runs: &[RunRecord]) -> String {
    if runs.is_empty() {
        return "This workflow has no recorded runs.".to_string();
    }
    let lines: Vec<String> = runs
        .iter()
        .map(|run| {
            let status = format!("{:?}", run.status).to_lowercase();
            let mut line = format!("- {} ({status})", run.id);
            if let Some(summary) = run
                .summary
                .as_deref()
                .map(str::trim)
                .filter(|s| !s.is_empty())
            {
                line.push_str(&format!(": {summary}"));
            }
            if run.status == RunStatus::Failed {
                if let Some(error) = run
                    .error
                    .as_deref()
                    .map(str::trim)
                    .filter(|e| !e.is_empty())
                {
                    line.push_str(&format!("\n  error: {error}"));
                }
            }
            for binding in run
                .diagnosis
                .iter()
                .flat_map(|diagnosis| diagnosis.null_bindings.iter())
            {
                line.push_str(&format!(
                    "\n  null binding: {} ({})",
                    binding.location, binding.expression
                ));
            }
            line
        })
        .collect();
    // `never_ran` is deliberately not reported. On a branching graph every run
    // leaves nodes unexecuted by design, and listing them here would have the
    // agent proposing fixes for conditions that are working correctly.
    format!(
        "{}\n\nCall `workflow_run_get` with a run id for its full record.",
        lines.join("\n")
    )
}
