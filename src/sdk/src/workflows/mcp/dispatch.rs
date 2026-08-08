//! Tool-call dispatch and the shared JSON helpers behind tool definitions.
//!
//! Names are prefixed by their family — `workflow_` and `fleet_` — so they
//! cannot collide with each other or with the harness's own reserved tool names
//! ([`crate::harness_contract::RESERVED_TOOL_NAMES`]).

use serde_json::{json, Value};

use crate::mcp::progress::Progress;
use crate::mcp::tools::{arg, content};
use crate::mcp::{McpSession, RpcError};
use crate::workflows::authoring::GraphHandle;
use crate::workflows::ops::{self, StepDetail, Wait};

use super::evolve::ToolMode;
use super::{run_detail, run_progress};

/// Every tool this server exposes, in the order a model meets them.
///
/// `workflow_run` really does run the thing — harness sessions, scripts, and
/// whatever else the graph describes. It is here because a copilot that can only
/// simulate cannot answer "does this work", and a dry run is structurally blind
/// to exactly the steps most worth checking: a `code` node's script, an
/// `agent` node's real reply. The operator's own switches still apply
/// (`workflows.enabled`, and the workflow's own `enabled`).
///
/// `workflow_run_cancel` reverses an earlier decision here. The reasoning was
/// that a copilot which started a run is awaiting it, and the operator cancels
/// from the pane where they can see it — which holds right up until the run was
/// started over MCP by a model that then answered with a run id and went away.
/// Nothing was watching that run in the pane, and the caller that started it
/// had no verb to stop it. Cancelling reaches the same process-local registry
/// the pane's key does, so this adds a caller rather than a mechanism.
pub const TOOL_NAMES: [&str; 21] = [
    "workflow_list",
    "workflow_get",
    "workflow_host",
    "workflow_catalog",
    "workflow_create",
    "workflow_apply_ops",
    "workflow_defaults",
    "workflow_preview_ops",
    "workflow_validate",
    "workflow_dry_run",
    "workflow_run",
    "workflow_runs",
    "workflow_run_get",
    "workflow_run_detail",
    "workflow_run_cancel",
    "workflow_history",
    "workflow_delete",
    "workflow_notes",
    "workflow_note_add",
    "workflow_proposals",
    "workflow_propose",
];

/// Whether a tool performs a short mutation of shared workflow authoring data.
///
/// Runs deliberately stay concurrent: they have unique run ids and may take
/// minutes. These authoring calls are short and include read-modify-write paths
/// that would otherwise lose one of two pipelined edits.
///
/// `workflow_run_cancel` is not one of them, despite the name reading like a
/// write. It mutates no stored definition — it flips a flag in the process-local
/// in-flight registry, which does its own locking — so the guard would buy no
/// safety, and taking it would queue a cancel behind whatever authoring call
/// happened to be in progress. A cancel is the one call whose whole value is
/// arriving promptly.
fn mutates_workflow(name: &str) -> bool {
    matches!(
        name,
        "workflow_create"
            | "workflow_apply_ops"
            | "workflow_defaults"
            | "workflow_delete"
            | "workflow_note_add"
            | "workflow_propose"
    )
}

/// Read the optional `inputs` argument: values for the workflow's declared
/// inputs, keyed by name.
///
/// Absent or `null` means "supplied none", which is correct for a workflow
/// whose inputs are all optional or defaulted. A present-but-non-object value
/// is the caller mis-shaping the call, and is rejected naming the argument —
/// left to fall through it would read as "you supplied nothing" and surface as
/// a confusing complaint about a required input the caller believes it sent.
fn declared_inputs(
    arguments: &Value,
    tool: &str,
) -> Result<serde_json::Map<String, Value>, RpcError> {
    match arguments.get("inputs") {
        None | Some(Value::Null) => Ok(serde_json::Map::new()),
        Some(Value::Object(map)) => Ok(map.clone()),
        Some(_) => Err(RpcError::invalid_params(format!(
            "{tool}: 'inputs' must be an object keyed by the workflow's declared input names, \
             e.g. {{\"repo\": \"acme/api\"}}"
        ))),
    }
}

/// How a caller asks to wait for a run, and what it means.
///
/// Async is the default because a real workflow outlives any client's idle
/// ceiling: `wait` is the opt-in for the short ones, and `waitMs` the middle
/// ground that holds the call open for as long as it is worth holding and then
/// hands back the run id rather than an error.
///
/// # Errors
///
/// A `waitMs` that is not a positive number is rejected; guessing what a
/// caller meant by `-1` or `"30s"` would be guessing how long to block for.
fn wait_mode(arguments: &Value) -> Result<Wait, RpcError> {
    if let Some(budget) = arguments.get("waitMs") {
        let millis = budget.as_u64().filter(|millis| *millis > 0).ok_or_else(|| {
            RpcError::invalid_params(
                "workflow_run: 'waitMs' must be a positive number of milliseconds to wait before                  answering with the run id",
            )
        })?;
        return Ok(Wait::Until(std::time::Duration::from_millis(millis)));
    }
    match arguments.get("wait") {
        Some(Value::Bool(true)) => Ok(Wait::Forever),
        _ => Ok(Wait::No),
    }
}

/// Read the optional `steps` argument: how much of each step to send back.
fn step_detail(arguments: &Value, default: StepDetail) -> Result<StepDetail, RpcError> {
    StepDetail::parse(arguments.get("steps").and_then(Value::as_str), default)
        .map_err(|err| RpcError::invalid_params(err.to_string()))
}

/// Run a `tools/call`.
///
/// `progress` is the client's notification channel for this call, when it asked
/// for one. Only a call that blocks can use it — a tool that has already
/// answered has nothing left to be making progress on.
pub(crate) async fn call(
    session: &McpSession,
    name: &str,
    arguments: Value,
    progress: Option<Progress>,
) -> Result<Value, RpcError> {
    let store = &session.store;
    let policy = &session.policy;
    let mode = session.mode;
    if let Some(error) = scope_error(mode, name, &arguments) {
        return Ok(content(&json!({ "error": error }), true));
    }

    let _mutation_guard = if mutates_workflow(name) {
        Some(session.workflow_mutations.lock().await)
    } else {
        None
    };

    let outcome = match name {
        "workflow_list" => ops::list(store).map_err(to_rpc),
        "workflow_get" => ops::get(store, arg(&arguments, "id")?).map_err(to_rpc),
        "workflow_catalog" => {
            ops::catalog(arguments.get("kind").and_then(Value::as_str)).map_err(to_rpc)
        }
        "workflow_create" => {
            let id = arg(&arguments, "id")?;
            ops::create(store, arg(&arguments, "document")?, id).map_err(to_rpc)
        }
        "workflow_apply_ops" => {
            let id = arg(&arguments, "id")?;
            let ops_value = arguments
                .get("ops")
                .ok_or_else(|| RpcError::invalid_params("missing required argument 'ops'"))?;
            ops::apply_ops(store, id, ops_value).map_err(to_rpc)
        }
        "workflow_preview_ops" => {
            let id = arg(&arguments, "id")?;
            let ops_value = arguments
                .get("ops")
                .ok_or_else(|| RpcError::invalid_params("missing required argument 'ops'"))?;
            ops::preview_ops(store, id, ops_value).map_err(to_rpc)
        }
        "workflow_validate" => {
            // Either handle, whichever the author gave: a saved id, or a
            // document they are still writing.
            let handle = match (
                arguments.get("id").and_then(Value::as_str),
                arguments.get("document").and_then(Value::as_str),
            ) {
                (Some(id), _) if !id.trim().is_empty() => GraphHandle::Saved(id),
                (_, Some(document)) if !document.trim().is_empty() => GraphHandle::Inline(document),
                _ => {
                    return Err(RpcError::invalid_params(
                        "workflow_validate: pass either 'id' or 'document'",
                    ))
                }
            };
            Ok(ops::validate(store, &handle))
        }
        "workflow_dry_run" => {
            let id = arg(&arguments, "id")?;
            let input = arguments.get("input").cloned().unwrap_or(json!({}));
            let inputs = declared_inputs(&arguments, "workflow_dry_run")?;
            ops::dry_run(store, id, input, inputs).await.map_err(to_rpc)
        }
        "workflow_run" => {
            let id = arg(&arguments, "id")?;
            let input = arguments.get("input").cloned().unwrap_or(json!({}));
            let inputs = declared_inputs(&arguments, "workflow_run")?;
            let wait = wait_mode(&arguments)?;
            let env: std::collections::HashMap<String, String> = std::env::vars().collect();
            let cwd = std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."));
            // Kept alongside the sink so a bounded wait that gives up on the
            // run can deactivate it below — the sink itself is moved into the
            // run's background task, which outlives this call.
            let progress = progress.filter(|_| wait.blocks());
            let result = ops::run(
                crate::workflows::local::LocalRun {
                    store: store.clone(),
                    config: &policy.workflows,
                    custom_harnesses: &policy.custom_harness_configs,
                    launch: &policy.launch,
                    env: &env,
                    cwd: &cwd,
                    // Where the run works, when the caller says. Refused inside
                    // `start` rather than here, so a bad path fails the same way
                    // whichever door started the run.
                    workspace: arguments
                        .get("workspace")
                        .and_then(Value::as_str)
                        .map(str::to_string),
                    workflow_id: id,
                    input: tinyflows::engine::RunInput::new(input).with_inputs(inputs),
                    // Only for a call that is holding itself open. A run that
                    // has already answered with its id would be reporting
                    // progress against a token the client stopped watching.
                    sink: progress
                        .clone()
                        .map(|progress| run_progress::sink(progress, id)),
                    liveness: Some(session.run_liveness.clone()),
                    // Whichever harness session this server is serving. Stamped
                    // on the record at admission, so a run that is still going
                    // is already attributable.
                    origin: session.origin.clone(),
                },
                wait,
            )
            .await
            .map_err(to_rpc);
            // `admitted`'s shape (`ops::run`, `Wait::Until` that ran out of
            // budget) is the only outcome where this call answers while the
            // run is still going — every other outcome either never made a
            // sink (`Wait::No`) or already ran to completion (`Wait::Forever`,
            // or `Wait::Until` that settled in time), so there is nothing left
            // to silence.
            if let (Ok(value), Some(progress)) = (&result, &progress) {
                if value.get("status").and_then(Value::as_str) == Some("running") {
                    progress.deactivate();
                }
            }
            result
        }
        // Counts by default: a history listing is read to find *which* run to
        // look at, and inlining every step of every run is what made the
        // cheapest question — "did my run finish?" — the most expensive answer.
        "workflow_runs" => ops::list_runs(
            store,
            arg(&arguments, "id")?,
            step_detail(&arguments, StepDetail::Counts)?,
        )
        .map_err(to_rpc),
        "workflow_run_get" => ops::get_run(
            store,
            arg(&arguments, "runId")?,
            step_detail(&arguments, StepDetail::Summary)?,
        )
        .map_err(to_rpc),
        "workflow_run_detail" => run_detail::detail(
            session,
            arg(&arguments, "runId")?,
            // Summary rather than counts: this tool is reached for when a run
            // needs explaining, and the elided half of a step is usually part
            // of the explanation.
            step_detail(&arguments, StepDetail::Summary)?,
        )
        .await
        .map_err(to_rpc),
        // No store read first to check the id exists. Cancelling is a race with
        // the run finishing by definition, and `cancel_run` already answers
        // "nothing here was executing that" in words — a preflight would only
        // add a second way to say the same thing, one call earlier.
        "workflow_run_cancel" => Ok(ops::cancel_run(arg(&arguments, "runId")?)),
        "workflow_defaults" => {
            let id = arg(&arguments, "id")?;
            let harness = arguments.get("harness").and_then(Value::as_str);
            let model = arguments.get("model").and_then(Value::as_str);
            if harness.is_none() && model.is_none() {
                return Err(RpcError::invalid_params(
                    "workflow_defaults: pass 'harness', 'model', or both; an empty string clears \
                     one",
                ));
            }
            ops::set_defaults(store, id, harness, model).map_err(to_rpc)
        }
        "workflow_host" => Ok(ops::host_facts(policy)),
        "workflow_history" => ops::list_history(store, arg(&arguments, "id")?).map_err(to_rpc),
        "workflow_delete" => ops::delete(store, arg(&arguments, "id")?).map_err(to_rpc),
        "workflow_notes" => ops::notes(store, arg(&arguments, "id")?).map_err(to_rpc),
        "workflow_note_add" => ops::add_note(
            store,
            arg(&arguments, "id")?,
            arg(&arguments, "kind")?,
            arg(&arguments, "text")?,
            string_list(&arguments, "runIds"),
            // Anything reaching this server over a tool call is a model. An
            // operator's own note comes in through the CLI or the TUI, and the
            // two are weighted differently in a brief.
            crate::workflows::NoteSource::Agent { model: None },
            string_list(&arguments, "supersedes"),
        )
        .map_err(to_rpc),
        "workflow_proposals" => ops::proposals(store, arg(&arguments, "id")?).map_err(to_rpc),
        "workflow_propose" => {
            let id = arg(&arguments, "id")?;
            let rationale = arg(&arguments, "rationale")?;
            let ops_value = arguments
                .get("ops")
                .ok_or_else(|| RpcError::invalid_params("missing required argument 'ops'"))?;
            ops::propose(
                store,
                id,
                rationale,
                ops_value,
                string_list(&arguments, "runIds"),
                string_list(&arguments, "noteIds"),
            )
            .await
            .map_err(to_rpc)
        }
        other => {
            return Err(RpcError::invalid_params(format!(
                "unknown tool '{other}'; available: {}",
                served(mode).join(", ")
            )))
        }
    };

    // MCP reports a *tool's* failure as content with `isError`, not as a
    // protocol error: the model should read what went wrong and try again,
    // rather than the client treating the call as broken.
    Ok(match outcome {
        Ok(value) => content(&value, false),
        Err(err) => content(&json!({ "error": err.message }), true),
    })
}

/// Refuse evolution writes that target a workflow other than the reviewed one.
fn scope_error(mode: ToolMode, name: &str, arguments: &Value) -> Option<String> {
    let scope = std::env::var(super::evolve::TOOL_SCOPE_ENV).ok();
    scope_error_for(mode, name, arguments, scope.as_deref())
}

/// Explain when an evolution write targets a workflow outside its review scope.
pub(crate) fn scope_error_for(
    mode: ToolMode,
    name: &str,
    arguments: &Value,
    scope: Option<&str>,
) -> Option<String> {
    if mode != ToolMode::Propose || !matches!(name, "workflow_note_add" | "workflow_propose") {
        return None;
    }
    let scope = scope?;
    let requested = arguments.get("id").and_then(Value::as_str).unwrap_or("");
    (requested != scope).then(|| {
        format!(
            "'{name}' is scoped to workflow '{scope}' for this review; \
             writing workflow '{requested}' is not allowed"
        )
    })
}

/// The tool names this mode serves.
pub fn served(mode: ToolMode) -> Vec<&'static str> {
    TOOL_NAMES
        .into_iter()
        .filter(|name| mode.allows(name))
        .collect()
}

/// An optional array-of-strings argument, absent or malformed alike as empty.
///
/// Forgiving on purpose: a model that sends a bare string instead of an array
/// has still told us something, and failing the whole call over the shape of an
/// optional provenance field costs a round trip to learn nothing.
fn string_list(arguments: &Value, name: &str) -> Vec<String> {
    match arguments.get(name) {
        Some(Value::Array(items)) => items
            .iter()
            .filter_map(|item| item.as_str().map(str::to_string))
            .collect(),
        Some(Value::String(single)) => vec![single.clone()],
        _ => Vec::new(),
    }
}

/// A workflow error as an RPC error.
fn to_rpc(err: crate::workflows::WorkflowError) -> RpcError {
    RpcError::invalid_params(err.to_string())
}
