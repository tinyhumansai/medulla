//! Run-local capture of the resolved prompts sent by `agent` nodes.
//!
//! The engine exposes a node's output to observers, but not the resolved config
//! it executed. Medulla tags an in-memory graph clone with each agent node id;
//! the harness capability consumes that private tag when it receives the
//! resolved request and records only the prompt. The authored graph and the
//! request sent to the harness are unchanged.

use tinyflows::model::{NodeKind, WorkflowGraph};

mod types;

pub(crate) use types::AgentEvidence;

#[cfg(test)]
mod tests;

/// Private config field used only on the compiled, in-memory graph clone.
pub(super) const NODE_ID_FIELD: &str = "__medulla_agent_node_id";

/// Clone `graph` and tag agent configs so resolved prompts can be correlated.
pub(crate) fn instrumented(graph: &WorkflowGraph) -> WorkflowGraph {
    let mut graph = graph.clone();
    for node in &mut graph.nodes {
        if node.kind != NodeKind::Agent {
            continue;
        }
        if let Some(config) = node.config.as_object_mut() {
            config.insert(
                NODE_ID_FIELD.to_string(),
                serde_json::Value::String(node.id.clone()),
            );
        }
    }
    graph
}
