//! The tool descriptions a harness reads before it acts.
//!
//! These are prose, not plumbing, and they are the entire briefing a model gets
//! about what each verb does and when not to reach for it — so they are kept
//! together, away from the dispatch, and edited as writing.
//!
//! The node-kind vocabulary is *generated* from the catalogue rather than
//! written out, so a description cannot drift from what the engine accepts.

use serde_json::{json, Value};

use super::dispatch::schema;
use super::evolve::ToolMode;
use crate::workflows::node_contracts::render_node_kinds_line;

/// The tool definitions a session in `mode` is served.
///
/// Filtered rather than merely documented: a turn that must not edit is not
/// shown the editing verbs at all, so the restriction holds without depending
/// on the model having read and believed a standing instruction.
pub fn tool_definitions(mode: ToolMode) -> Vec<Value> {
    all_definitions()
        .into_iter()
        .filter(|tool| {
            tool.get("name")
                .and_then(Value::as_str)
                .is_some_and(|name| mode.allows(name))
        })
        .collect()
}

/// Every tool, before any mode filtering.
fn all_definitions() -> Vec<Value> {
    let mut tools = vec![
        json!({
            "name": "workflow_list",
            "description":
                "List the workflows installed on this machine. Start here: a workflow is a saved \
                 multi-step plan whose `agent` steps each run on a real coding harness.",
            "inputSchema": schema(json!({}), &[]),
        }),
        json!({
            "name": "workflow_get",
            "description":
                "Fetch one workflow whole, including the graph you would edit. Call this before \
                 workflow_apply_ops so your patches target node ids that exist.",
            "inputSchema": schema(
                json!({ "id": { "type": "string", "description": "The workflow id." } }),
                &["id"],
            ),
        }),
        json!({
            "name": "workflow_host",
            "description":
                "What this machine will actually permit a workflow to do: the default worker \
                 `agent` nodes dispatch to, the tool slugs and HTTP hosts that are allowed, and \
                 whether `code` nodes may run. Read this before writing a graph that reaches \
                 outside the process — every one of these is enforced at run time, so a graph \
                 that ignores them saves and validates cleanly and then fails the first time it \
                 matters.",
            "inputSchema": schema(json!({}), &[]),
        }),
        json!({
            "name": "workflow_catalog",
            "description": format!(
                "The node kinds a workflow may use, with this host's own notes on each. Read \
                 this before writing a graph.\n\n{}",
                render_node_kinds_line()
            ),
            "inputSchema": schema(
                json!({
                    "kind": {
                        "type": "string",
                        "description":
                            "Narrow to one node kind. Omit for every contract.",
                    }
                }),
                &[],
            ),
        }),
        json!({
            "name": "workflow_create",
            "description":
                "Install a workflow from a whole graph document. The document is a tinyflows \
                 WorkflowGraph — `nodes` and `edges`, plus optional `name`, `description`, and \
                 `enabled`. Exactly one node must be a `trigger`. An invalid graph is refused \
                 and nothing is written, so validate first if you are unsure.",
            "inputSchema": schema(
                json!({
                    "id": { "type": "string", "description": "The id to install it under." },
                    "document": {
                        "type": "string",
                        "description": "The workflow graph as a JSON string.",
                    }
                }),
                &["id", "document"],
            ),
        }),
        json!({
            "name": "workflow_apply_ops",
            "description":
                "Edit a saved workflow with graph patches, and save the result. Prefer this over \
                 rewriting a whole document: each op is checked, and a batch that fails anywhere \
                 leaves the workflow untouched. Ops are objects like \
                 {\"op\":\"update_node_config\",\"id\":\"build\",\"config\":{...}} — \
                 update_node_config is an RFC 7386 merge patch, so a null leaf deletes a key. \
                 Note that rename_node rewires edges but does NOT rewrite `=nodes.<id>` \
                 expressions inside other nodes' configs; re-point those yourself.",
            "inputSchema": schema(
                json!({
                    "id": { "type": "string", "description": "The workflow to edit." },
                    "ops": { "type": "array", "description": "The graph ops to apply." }
                }),
                &["id", "ops"],
            ),
        }),
        json!({
            "name": "workflow_preview_ops",
            "description":
                "Check graph patches against a saved workflow without saving them. The safe way \
                 to find out whether an edit is sound.",
            "inputSchema": schema(
                json!({
                    "id": { "type": "string", "description": "The workflow to check against." },
                    "ops": { "type": "array", "description": "The graph ops to preview." }
                }),
                &["id", "ops"],
            ),
        }),
        json!({
            "name": "workflow_validate",
            "description":
                "Validate a saved workflow, or a document you have not saved yet. Reports every \
                 problem at once rather than the first, so one call tells you everything wrong. \
                 Returns ok:false with errors rather than failing.",
            "inputSchema": schema(
                json!({
                    "id": { "type": "string", "description": "A saved workflow to validate." },
                    "document": {
                        "type": "string",
                        "description": "An unsaved graph document to validate instead.",
                    }
                }),
                &[],
            ),
        }),
        json!({
            "name": "workflow_dry_run",
            "description":
                "Simulate a saved workflow: every expression is resolved and every declared \
                 output shape satisfied, but no harness session is started and nothing outside \
                 this process is touched. Catches wiring mistakes that validation cannot — an \
                 expression pointing at a node that produces nothing still validates.",
            "inputSchema": schema(
                json!({
                    "id": { "type": "string", "description": "The workflow to simulate." },
                    "input": { "description": "Optional trigger payload; defaults to {}." }
                }),
                &["id"],
            ),
        }),
        json!({
            "name": "workflow_run",
            "description":
                "Run a workflow for real: dispatch its `agent` steps to actual coding harnesses, \
                 execute its scripts, and make whatever changes it describes. This is not a \
                 simulation — prefer workflow_dry_run while you are still wiring, and use this \
                 when the operator has asked whether it works, or when a dry run cannot settle \
                 the question (a `code` node's script and an `agent` node's real reply are both \
                 invisible to one). Returns the whole run record: every step, its status, and \
                 anything that resolved to null. Can take minutes.",
            "inputSchema": schema(
                json!({
                    "id": { "type": "string", "description": "The workflow to run." },
                    "input": { "description": "Optional trigger payload; defaults to {}." }
                }),
                &["id"],
            ),
        }),
        json!({
            "name": "workflow_runs",
            "description":
                "The run history for a workflow, newest first — status, steps, and anything a \
                 run is waiting for approval on.",
            "inputSchema": schema(
                json!({ "id": { "type": "string", "description": "The workflow id." } }),
                &["id"],
            ),
        }),
        json!({
            "name": "workflow_run_get",
            "description":
                "One run in full, by run id: every step, its status, its duration, and the \
                 expressions that resolved to null on the way. Where to start when asked why a \
                 workflow failed — the steps say what actually happened, which is more reliable \
                 than reading the graph and reasoning about what it would do.",
            "inputSchema": schema(
                json!({ "runId": { "type": "string", "description": "The run id." } }),
                &["runId"],
            ),
        }),
        json!({
            "name": "workflow_history",
            "description":
                "The versions of a workflow that have been written over, newest first, each with \
                 the whole graph as it then was. Useful for saying what changed and when — an \
                 edit that broke something is often easier to see next to the version before it. \
                 Restoring one is the operator's own action, not yours.",
            "inputSchema": schema(
                json!({ "id": { "type": "string", "description": "The workflow id." } }),
                &["id"],
            ),
        }),
        json!({
            "name": "workflow_delete",
            "description":
                "Remove a workflow. Only when the operator asked for it in this turn — deleting \
                 something they did not ask about is not a helpful tidy-up. The version is kept \
                 in history, so an operator can undo it, but do not treat that as licence.",
            "inputSchema": schema(
                json!({ "id": { "type": "string", "description": "The workflow to remove." } }),
                &["id"],
            ),
        }),
    ];
    tools.extend(super::evolve::definitions(schema));
    tools
}
