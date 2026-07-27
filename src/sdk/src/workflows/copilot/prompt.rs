//! The instruction the copilot harness is given.
//!
//! Medulla drives harnesses as an ACP *client*, so a copilot turn is one
//! dispatched task and the whole of what the agent knows is the prompt plus the
//! `medulla-workflows` MCP tools already attached to the session
//! ([`crate::workflows::mcp`]). There is no system-prompt channel to put the
//! rules in, so they go here, in front of the operator's instruction.
//!
//! Three things have to be established every turn: *which* workflow is being
//! edited (the harness can see all of them and would otherwise have to guess),
//! that edits go through the tools rather than by writing files (the store's
//! layering means the file the agent would find is not necessarily the record
//! the operator is looking at), and that the graph must be left valid.

use tinyflows::model::WorkflowGraph;

/// How much of the current graph is pasted into the prompt.
///
/// A large graph would crowd out the instruction and cost a round trip's worth
/// of tokens on something the agent can fetch itself. Past this it gets the
/// summary and is told to call `workflow_get` — which it needs to do anyway
/// before patching, so nothing is lost.
const MAX_INLINE_GRAPH_BYTES: usize = 6000;

/// The rules, in front of every turn.
///
/// Written as instructions to the agent rather than as description, because that
/// is what a harness acts on.
const PREAMBLE: &str = "\
You are the workflow copilot inside Medulla's terminal UI. The operator is \
looking at one workflow's graph and has asked you to change or explain it.

Use the `medulla-workflows` MCP tools for everything:

- `workflow_get` to read the graph whole before you patch it.
- `workflow_catalog` to check a node kind's config shape — do not guess it.
- `workflow_preview_ops` to check a patch, then `workflow_apply_ops` to save it.
- `workflow_validate` and `workflow_dry_run` to confirm the result still works.

Rules:

- Edit ONLY the workflow named below, and only through the tools. Do not write \
workflow files directly: the store is layered and the file you would find is \
not necessarily the record the operator is looking at.
- Patch with ops rather than re-creating the workflow. A rewrite loses fields \
you were not asked to change.
- Leave the graph valid. If an edit cannot be made valid, make no edit and say \
what is in the way.
- If the operator asked a question rather than for a change, answer it and \
change nothing.
- Reply in at most three sentences, in plain prose: what you changed, or why \
you did not. The operator sees the graph beside your reply, so do not restate \
it or paste JSON.";

/// Build the prompt for one copilot turn.
///
/// `instruction` is the operator's own words, passed through unaltered and
/// placed last — a harness weights the end of a prompt most heavily, and the
/// instruction is the part that changes.
pub fn build(workflow_id: &str, name: &str, graph: &WorkflowGraph, instruction: &str) -> String {
    let mut prompt = String::with_capacity(MAX_INLINE_GRAPH_BYTES + 2048);
    prompt.push_str(PREAMBLE);
    prompt.push_str("\n\n## The workflow\n\n");
    prompt.push_str(&format!("id: {workflow_id}\nname: {name}\n"));
    prompt.push_str(&format!(
        "{} node{}, {} edge{}\n\n",
        graph.nodes.len(),
        if graph.nodes.len() == 1 { "" } else { "s" },
        graph.edges.len(),
        if graph.edges.len() == 1 { "" } else { "s" },
    ));
    prompt.push_str(&graph_section(graph));
    prompt.push_str("\n\n## The instruction\n\n");
    prompt.push_str(instruction.trim());
    prompt.push('\n');
    prompt
}

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
#[path = "prompt_tests.rs"]
mod tests;
