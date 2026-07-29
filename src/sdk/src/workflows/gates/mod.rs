//! Checks that run before an authoring write lands.
//!
//! The engine's own [`validate`](tinyflows::validate) answers "would this
//! compile" — no trigger, an edge to a node that is not there. That is a real
//! bar and it is not the one authors keep failing. The graphs that cost people
//! an afternoon *do* compile: they have a binding that resolves to null at run
//! time, so a step executes with an empty value and the run reports success
//! having done nothing.
//!
//! Nothing downstream catches that. A null is a legal value, so the engine has
//! no complaint; the run record shows every node green. The only place it can be
//! caught is here, before the write, while there is still an author on the other
//! end to tell.
//!
//! Two rules the gates hold themselves to:
//!
//! - **Refuse only what is *guaranteed* wrong.** A gate that fires on a graph
//!   that would have worked costs an author their edit and teaches them to
//!   distrust the tool. Everything merely suspicious belongs in the dry run's
//!   diagnostics ([`crate::workflows::ops::dry_run`]), which advise rather than
//!   refuse.
//! - **Say what to do.** Every message names the node, the binding, and the
//!   correction. The reader is usually an agent with one round trip to spend.
//!
//! Adapted from the sibling `openhuman` host's gate stack, minus the parts that
//! are about its integration registry rather than about graphs.

mod bindings;

pub use bindings::{collect_expressions, parse_node_binding, reads_as_prose, NodeBinding};

use tinyflows::model::{NodeKind, WorkflowGraph};

use crate::workflows::WorkflowError;

/// Run every gate, collecting all failures rather than the first.
///
/// One round trip then tells an author everything wrong with what they wrote,
/// which matters most when the author is an agent editing over a tool call.
///
/// # Errors
///
/// Returns [`WorkflowError::Invalid`] listing every failure. An empty result is
/// a pass.
pub fn check(id: &str, graph: &WorkflowGraph) -> Result<(), WorkflowError> {
    let messages = failures(graph);
    if messages.is_empty() {
        return Ok(());
    }
    Err(WorkflowError::Invalid {
        id: id.to_string(),
        messages,
    })
}

/// Every gate failure in `graph`.
pub fn failures(graph: &WorkflowGraph) -> Vec<String> {
    let mut failures = agent_prompt_failures(graph);
    failures.extend(binding_failures(graph));
    failures.extend(code_language_failures(graph));
    failures
}

/// `code` nodes whose language the engine will not read the way it was written.
///
/// The engine matches the literal string `"python"` and treats *everything else*
/// as JavaScript — silently. So `"language": "python3"` runs a Python program
/// through node, and `"language": "shell"` runs a shell script through node.
/// Both fail with a syntax error from an interpreter the author never named,
/// which is among the least helpful failures this host can produce.
///
/// Refused here rather than documented, because documentation does not stop a
/// plausible spelling.
fn code_language_failures(graph: &WorkflowGraph) -> Vec<String> {
    // The two the engine actually distinguishes. Not `ScriptLanguage::NAMES`:
    // that includes `shell`, which this node kind cannot reach.
    const ACCEPTED: [&str; 2] = ["javascript", "python"];

    let mut failures = Vec::new();
    for node in &graph.nodes {
        if node.kind != NodeKind::Code {
            continue;
        }
        let Some(language) = node.config.get("language").and_then(|v| v.as_str()) else {
            // Absent is legal and means JavaScript, which the engine's own
            // default already says.
            continue;
        };
        if ACCEPTED.contains(&language) {
            continue;
        }
        let hint = if crate::flow_engine::caps::script::ScriptLanguage::parse(language)
            == Some(crate::flow_engine::caps::script::ScriptLanguage::Shell)
        {
            " A `code` node cannot run shell: use a `tool_call` with the `medulla:shell` slug, \
             which runs in the operator's project directory."
        } else {
            ""
        };
        failures.push(format!(
            "node '{}': `language` is `{language}`, which this engine does not recognise — it \
             matches only the exact strings `javascript` and `python`, and silently treats \
             anything else as JavaScript. Your program would be run through node and fail with a \
             syntax error naming an interpreter you did not choose.{hint}",
            node.id
        ));
    }
    failures
}

/// Agent nodes whose `prompt` is prose written as an expression.
///
/// The node would run with an empty instruction — for Medulla that means
/// dispatching a whole harness session with nothing to do.
fn agent_prompt_failures(graph: &WorkflowGraph) -> Vec<String> {
    let mut failures = Vec::new();
    for node in &graph.nodes {
        if node.kind != NodeKind::Agent {
            continue;
        }
        // `instruction` is the alias the rest of Medulla uses for the same
        // field, and the engine accepts it, so it has to be checked too.
        for key in ["prompt", "instruction"] {
            let Some(text) = node.config.get(key).and_then(|v| v.as_str()) else {
                continue;
            };
            if !tinyflows::expr::is_expression(text) {
                continue;
            }
            if reads_as_prose(text[1..].trim()) {
                failures.push(format!(
                    "node '{}': `{key}` (`{text}`) reads as an instruction written as a \
                     `=`-expression, not as a jq program. `=` does not interpolate — the whole \
                     thing resolves to null and the node dispatches a harness session with an \
                     empty prompt. Fix: drop the leading `=` and write the instruction plainly, \
                     referring to upstream data with a separate `=` binding.",
                    node.id
                ));
            }
        }
    }
    failures
}

/// Bindings that read a node's output through the wrong shape.
fn binding_failures(graph: &WorkflowGraph) -> Vec<String> {
    let mut failures = Vec::new();
    for node in &graph.nodes {
        for (location, expr) in collect_expressions(&node.config) {
            let Some(binding) = parse_node_binding(&expr) else {
                continue;
            };
            // A binding to a node that does not exist is the engine's to
            // report, and it already does.
            let Some(target) = bindings::node_of(graph, &binding.node_id) else {
                continue;
            };
            if bindings::wraps_output(&target.kind) && !binding.through_envelope {
                failures.push(format!(
                    "node '{}': `{location}` (`{expr}`) reads `.item.{path}` from {article} node \
                     `{target_id}`, whose output is wrapped as {{json, text, raw}} — so this \
                     resolves to null at run time and the step gets nothing. Fix: \
                     `=nodes.{target_id}.item.json.{path}`.",
                    node.id,
                    path = binding.field_path,
                    article = bindings::kind_article(&target.kind),
                    target_id = binding.node_id,
                ));
            }
        }
    }
    failures
}

#[cfg(test)]
mod tests;
