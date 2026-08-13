//! The tool descriptions a harness reads before it acts.
//!
//! These are prose, not plumbing, and they are the entire briefing a model gets
//! about what each verb does and when not to reach for it — so they are kept
//! together, away from the dispatch, and edited as writing.
//!
//! The node-kind vocabulary is *generated* from the catalogue rather than
//! written out, so a description cannot drift from what the engine accepts.

use serde_json::{json, Value};

use crate::mcp::tools::schema;
use crate::workflows::node_contracts::render_node_kinds_line;

/// The tool definitions this session is served.
///
/// Filtered rather than merely documented: a turn that must not edit is not
/// shown the editing verbs at all, and a session whose grant does not cover a
/// family is not shown that family — so both restrictions hold without
/// depending on the model having read and believed a standing instruction.
pub(crate) fn definitions() -> Vec<Value> {
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
                 WorkflowGraph — `nodes` and `edges`, plus optional `name`, `description`, \
                 `enabled`, and a `defaults` block pinning the harness and model its steps run \
                 on. Exactly one node must be a `trigger`. An invalid graph is refused \
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
            "name": "workflow_defaults",
            "description":
                "Set what every `agent` step in a workflow runs on unless the step says \
                 otherwise: `harness` (one of the built-in coding CLIs, or a custom preset id — \
                 see workflow_host) and `model`. This is a property of the workflow, not of its \
                 graph, so it is set here rather than with a graph op. Pass an empty string to \
                 clear one; omit a field to leave it as it is. A step that names its own \
                 `config.harness` still wins.",
            "inputSchema": schema(
                json!({
                    "id": { "type": "string", "description": "The workflow to set." },
                    "harness": {
                        "type": "string",
                        "description":
                            "The harness every step defaults to. Empty string clears it.",
                    },
                    "model": {
                        "type": "string",
                        "description":
                            "The model hint every step defaults to. Empty string clears it.",
                    }
                }),
                &["id"],
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
                    "input": { "description": "Optional trigger payload; defaults to {}." },
                    "inputs": { "type": "object", "description": "Values for the workflow's declared inputs, keyed by name. Read the declarations from workflow_get / workflow_list; a missing required value, a wrong type, or a name the workflow does not declare is rejected and nothing runs." }
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
                 invisible to one). Starts the run and answers immediately with its runId; the \
                 run keeps going without this call. Poll workflow_run_get with that id for its \
                 status and, once it is settled, what it did. A real workflow takes minutes to \
                 hours — a single `agent` step is a whole coding session — so do not wait on one \
                 unless you know it is short.",
            "inputSchema": schema(
                json!({
                    "id": { "type": "string", "description": "The workflow to run." },
                    "input": { "description": "Optional trigger payload; defaults to {}." },
                    "inputs": { "type": "object", "description": "Values for the workflow's declared inputs, keyed by name. Read the declarations from workflow_get / workflow_list; a missing required value, a wrong type, or a name the workflow does not declare is rejected and nothing runs." },
                    "workspace": { "type": "string", "description": "The directory this run works in: where its shell steps run, the root their `args.cwd` and `args.script_path` resolve inside, and the checkout every `agent` step's harness opens. Absolute, or relative to where this server was started (`~` is expanded); defaults to that directory. This is how you point a workflow at another repository — do not smuggle a path in through a declared input, because a step's own `cwd` may not escape the workspace. A path that is not a directory on this host is refused and nothing runs." },
                    "wait": { "type": "boolean", "description": "Hold this call open until the run settles and answer with the whole run record. Only for workflows you know finish in a couple of minutes; anything longer will be aborted by your own idle timeout while the run carries on regardless." },
                    "waitMs": { "type": "integer", "description": "Wait up to this many milliseconds, then answer with the runId if the run is still going. Takes precedence over `wait`, and is the safe way to wait: a run that outlives the budget is reported as running, not as a failure." }
                }),
                &["id"],
            ),
        }),
        json!({
            "name": "workflow_runs",
            "description":
                "The run history for a workflow, newest first — status, timestamps, step count, \
                 and anything a run is waiting for approval on. Step inputs and outputs are not \
                 inlined: this answers which runs exist and how each ended, and workflow_run_get \
                 answers what one of them did.",
            "inputSchema": schema(
                json!({
                    "id": { "type": "string", "description": "The workflow id." },
                    "steps": { "type": "string", "enum": ["counts", "summary", "full"], "description": "How much of each run's steps to include. Defaults to 'counts' — no steps at all. 'full' inlines every prompt and output for every run and is rarely what you want here." }
                }),
                &["id"],
            ),
        }),
        json!({
            "name": "workflow_run_get",
            "description":
                "One run by id: its status, every step, each step's duration, and the \
                 expressions that resolved to null on the way. Where to start when asked why a \
                 workflow failed — the steps say what actually happened, which is more reliable \
                 than reading the graph and reasoning about what it would do. Also how you \
                 follow a run you started: the record exists from the moment the run is \
                 admitted, so calling this while it is still `running` shows how far it has got.",
            "inputSchema": schema(
                json!({
                    "runId": { "type": "string", "description": "The run id." },
                    "steps": { "type": "string", "enum": ["summary", "full", "counts"], "description": "How much of each step to include. Defaults to 'summary' — node, status, duration, diagnostics, and a bounded preview of the output. Ask for 'full' when a truncated output is the thing you need to read; it can be very large." }
                }),
                &["runId"],
            ),
        }),
        json!({
            "name": "workflow_run_detail",
            "description":
                "Everything workflow_run_get says about a run, plus what the fleet is doing for \
                 it right now. Reach for this when a run has been `running` longer than it \
                 should and the steps do not explain why — the store only learns about a step \
                 once it has finished, so an `agent` node in its twentieth minute is invisible \
                 to workflow_run_get by construction, and this is where it becomes visible. \
                 `live.harnesses` lists each harness session currently dispatched for this run, \
                 with the worker running it and the task id (`wf:<runId>:<route>#<n>`) to quote \
                 into a log. `live.executingHere` says whether this server is the process \
                 running it, which is also whether workflow_run_cancel can stop it. What you \
                 will not get is the harness's own transcript: this can tell you a worker is \
                 still working on a step, not what it is typing. An empty `live.harnesses` on a \
                 run that is still going means it is between steps or on an in-process node — \
                 read `live.note`, which says which case it is.",
            "inputSchema": schema(
                json!({
                    "runId": { "type": "string", "description": "The run id." },
                    "steps": { "type": "string", "enum": ["summary", "full", "counts"], "description": "How much of each finished step to include; the live half is unaffected. Defaults to 'summary'. Ask for 'counts' when the live harness view is the only thing you came for." }
                }),
                &["runId"],
            ),
        }),
        json!({
            "name": "workflow_run_cancel",
            "description":
                "Stop a run that is still going. Use it on a run you started and no longer want \
                 — the operator changed their mind, the inputs were wrong, it is looping — \
                 rather than leaving a harness session burning for another twenty minutes. Not \
                 a tidy-up: a run someone else started is theirs, and a run you merely find in \
                 the history is already over. Only reaches runs executing in this same process, \
                 which is the process that served the workflow_run that started them; a run \
                 started from Medulla's own pane or from another shell answers \
                 cancelled:false with the reason, and is not an error to retry. Check \
                 workflow_run_detail's `live.executingHere` first if you want to know before \
                 asking.",
            "inputSchema": schema(
                json!({ "runId": { "type": "string", "description": "The run to stop." } }),
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
