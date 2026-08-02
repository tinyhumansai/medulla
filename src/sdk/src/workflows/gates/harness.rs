//! The gate over an `agent` node's choice of harness and model.
//!
//! Both failures this catches are silent at run time in the worst way. A
//! misspelled harness would otherwise fail the node minutes into a run, after
//! the steps before it have already had their effects; and a harness written as
//! a `=`-expression would let whatever flowed down the graph decide which binary
//! and which credentials run the next instruction.
//!
//! The second is why this gate exists at all rather than being left to the
//! dispatch path: by the time a node dispatches, the engine has already resolved
//! its config, so an expression is indistinguishable from a name typed by hand.
//! Authoring time is the only place the difference is still visible.

use tinyflows::model::{NodeKind, WorkflowGraph};

use crate::flow_engine::HarnessSelector;

/// The config keys an `agent` node may name a harness under. Mirrors the
/// dispatch path's own list; a key accepted there and unchecked here would be
/// the one way past this gate.
const HARNESS_KEYS: [&str; 2] = ["harness", "provider"];

/// Every failure in `graph`'s harness and model selection.
pub fn failures(graph: &WorkflowGraph) -> Vec<String> {
    let mut failures = Vec::new();
    for node in &graph.nodes {
        if node.kind != NodeKind::Agent {
            continue;
        }
        for key in HARNESS_KEYS {
            let Some(value) = node.config.get(key) else {
                continue;
            };
            if value.is_null() {
                continue;
            }
            let Some(text) = value.as_str() else {
                failures.push(format!(
                    "node '{}': `{key}` must be a string naming a harness — one of {}, or the id \
                     of a custom harness preset. Omit it to inherit the workflow's `defaults` or \
                     the host's configured harness.",
                    node.id,
                    HarnessSelector::builtin_names().join(", ")
                ));
                continue;
            };
            if tinyflows::expr::is_expression(text) {
                failures.push(format!(
                    "node '{}': `{key}` (`{text}`) is a `=`-expression. Which harness runs a step \
                     is a decision the graph makes, not one its data makes — an expression here \
                     would let upstream output, including a model's own, choose which binary and \
                     which credentials run the next instruction. Fix: write the harness name \
                     plainly.",
                    node.id
                ));
                continue;
            }
            // Blank means "say nothing", which the dispatch path accepts, so it
            // is not a failure here either.
            if text.trim().is_empty() {
                continue;
            }
            if let Err(message) = HarnessSelector::parse(text) {
                failures.push(format!("node '{}': {message}", node.id));
            }
        }
    }
    failures
}
