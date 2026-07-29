//! The sections of a brief that describe the *situation* rather than the task.
//!
//! Everything here answers "what is the agent looking at": the graph as it
//! stands today, and — as the host learns more about a workflow — whatever else
//! grounds the turn. These grow independently of the turn structure in the
//! parent module, which is why they are apart from it.

use tinyflows::model::WorkflowGraph;

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
