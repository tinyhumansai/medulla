//! Task request construction, workflow validation, and lifecycle operations.

use std::sync::Arc;
use std::time::Duration;

use serde_json::{json, Value};
use uuid::Uuid;

use crate::hub::TaskRequest;
use crate::tinyplace::HarnessProvider;

use super::super::super::grants::Grant;
use super::super::super::types::{ControlFailure, ErrorKind, FleetOps};
use super::super::types::{SpawnError, TaskEntry, TaskRegistry, TaskState};
use super::params::{optional_str, required_str};
use super::workers::{probe_worker_workflows, resolve_worker};

/// The longest a `task.get` may block before answering "still running".
pub const MAX_WAIT: Duration = Duration::from_secs(120);

/// Render one task entry for the control wire.
pub(super) fn task_json(entry: &TaskEntry) -> Value {
    let mut value = json!({
        "taskId": entry.task_id,
        "worker": entry.worker,
        "instruction": entry.instruction,
        "status": entry.state.status(),
        "startedAt": entry.started_at,
        "elapsedMs": entry.elapsed_ms(),
    });
    let object = value.as_object_mut().expect("a json object");
    if let Some(finished) = entry.finished_at {
        object.insert("finishedAt".into(), json!(finished));
    }
    match &entry.state {
        TaskState::Running => {}
        TaskState::Done(outcome) => {
            object.insert("reply".into(), json!(outcome.reply));
            object.insert(
                "usage".into(),
                json!({
                    "inputTokens": outcome.usage.input_tokens,
                    "outputTokens": outcome.usage.output_tokens,
                }),
            );
            if let Some(harness) = outcome.harness {
                object.insert("harness".into(), json!(harness.as_str()));
            }
        }
        TaskState::Failed {
            message, retryable, ..
        } => {
            object.insert("error".into(), json!(message));
            object.insert("retryable".into(), json!(retryable));
        }
    }
    if !entry.status_tail.is_empty() {
        object.insert("progress".into(), json!(entry.status_tail));
    }
    value
}

/// Build the dispatch request for an accepted `task.dispatch`.
///
/// Task and abort ids are minted here because both are capability handles. A
/// caller-selected abort id could cancel work it does not own, while a reused
/// worker task id would silently collapse two dispatches into one.
fn build_request(
    grant: &Grant,
    params: &Value,
    worker_address: String,
) -> Result<TaskRequest, ControlFailure> {
    let instruction = required_str(params, "instruction")?;
    let workflow = optional_str(params, "workflow");
    let workflow_inputs = match params.get("inputs") {
        None => serde_json::Map::new(),
        Some(Value::Object(inputs)) => inputs.clone(),
        Some(_) => {
            return Err(ControlFailure::new(
                ErrorKind::BadRequest,
                "`inputs` must be an object keyed by declared workflow input name",
            ))
        }
    };
    if workflow.is_none() && !workflow_inputs.is_empty() {
        return Err(ControlFailure::new(
            ErrorKind::BadRequest,
            "`inputs` may only be supplied when `workflow` names a saved workflow",
        ));
    }
    if grant.tool_mode.is_some() && workflow.is_some() {
        return Err(ControlFailure::new(
            ErrorKind::BadRequest,
            "a proposal-only session may delegate a review instruction, but cannot run a saved \
             workflow through the fleet",
        ));
    }
    if workflow.is_some()
        && (optional_str(params, "harness").is_some() || optional_str(params, "model").is_some())
    {
        return Err(ControlFailure::new(
            ErrorKind::BadRequest,
            "`harness` and `model` apply to direct harness tasks, not saved workflows; configure \
             routing on the workflow or its agent nodes",
        ));
    }
    let provider = match optional_str(params, "harness") {
        Some(name) => Some(HarnessProvider::from_wire(&name).ok_or_else(|| {
            ControlFailure::new(
                ErrorKind::BadRequest,
                format!("`{name}` is not a harness; use claude, codex, or opencode"),
            )
        })?),
        None => None,
    };
    Ok(TaskRequest {
        task_id: format!("mcp-{}", Uuid::new_v4()),
        abort_id: format!("mcp-{}", Uuid::new_v4()),
        cycle_id: Some(format!("mcp:{}", grant.session)),
        instruction,
        worker_address,
        provider,
        custom_harness: None,
        model: optional_str(params, "model"),
        // The server-side grant preserves proposal-only scope across delegation.
        tool_mode: grant.tool_mode.clone(),
        workflow,
        workflow_fingerprint: optional_str(params, "workflowFingerprint"),
        workflow_inputs,
        // Context-free tasks may run concurrently without sharing transcript state.
        conversation: None,
        // Depth comes from the grant so callers cannot forge a shallower child.
        fleet_depth: grant.child_depth(),
    })
}

/// Accept a dispatch and return its registry handle.
pub(super) async fn task_dispatch(
    ops: &Arc<dyn FleetOps>,
    registry: &TaskRegistry,
    token: &str,
    grant: &Grant,
    params: &Value,
) -> Result<Value, ControlFailure> {
    // This repeats the tool-list restriction as a backstop for direct calls.
    if !grant.families.fleet {
        return Err(ControlFailure::new(
            ErrorKind::Unauthenticated,
            "this session was not granted the fleet tools; \
             the operator turned them off for this host",
        ));
    }
    if !grant.may_dispatch() {
        return Err(ControlFailure::new(
            ErrorKind::DepthExceeded,
            format!(
                "you are {} level(s) deep in a dispatch tree and the limit is {}; \
                 fanning out further would run with no operator in the loop — \
                 do the work, or report what you need",
                grant.depth, grant.max_depth
            ),
        ));
    }
    let worker = resolve_worker(ops, optional_str(params, "worker").as_deref())?;
    let request = build_request(grant, params, worker.clone())?;
    validate_worker_workflow(ops, &worker, params).await?;
    let task_id = match registry
        .spawn_below_limit(ops.clone(), token.to_string(), request, grant.max_in_flight)
        .await
    {
        Ok(task_id) => task_id,
        Err(SpawnError::AtCapacity(in_flight)) => {
            return Err(ControlFailure::new(
                ErrorKind::TooManyInFlight,
                format!(
                    "you already have {in_flight} task(s) running, which is this session's limit; \
                     wait for one to finish before dispatching another"
                ),
            ));
        }
        Err(SpawnError::GlobalAtCapacity(in_flight)) => {
            return Err(ControlFailure::new(
                ErrorKind::TooManyInFlight,
                format!(
                    "this Medulla instance already has {in_flight} fleet tasks running; wait for \
                     one to finish before dispatching another"
                ),
            ));
        }
        Err(SpawnError::Unavailable) => {
            return Err(ControlFailure::new(
                ErrorKind::Internal,
                "the task registry is unavailable",
            ));
        }
    };
    Ok(json!({ "taskId": task_id, "worker": worker, "status": "running" }))
}

/// Bind a workflow dispatch to the selected worker's advertised definition.
async fn validate_worker_workflow(
    ops: &Arc<dyn FleetOps>,
    worker: &str,
    params: &Value,
) -> Result<(), ControlFailure> {
    let workflow = optional_str(params, "workflow");
    let fingerprint = optional_str(params, "workflowFingerprint");
    let Some(workflow) = workflow else {
        if fingerprint.is_some() {
            return Err(ControlFailure::new(
                ErrorKind::BadRequest,
                "`workflowFingerprint` may only be supplied with `workflow`",
            ));
        }
        return Ok(());
    };
    let fingerprint = fingerprint.ok_or_else(|| {
        ControlFailure::new(
            ErrorKind::BadRequest,
            "a workflow dispatch requires `workflowFingerprint` from the selected worker's \
             entry in `fleet_workers`; local workflow ids are not portable across machines",
        )
    })?;
    let catalog = probe_worker_workflows(ops, worker).await.map_err(|error| {
        ControlFailure::new(
            ErrorKind::DispatchFailed,
            format!("could not verify workflows on worker {worker}: {error}"),
        )
    })?;
    let advertised = catalog.iter().find(|candidate| candidate.id == workflow);
    match advertised {
        None => Err(ControlFailure::new(
            ErrorKind::BadRequest,
            format!(
                "worker {worker} does not advertise workflow `{workflow}`; call fleet_workers and \
                 choose from that worker's workflow catalog"
            ),
        )),
        Some(advertised) if advertised.fingerprint != fingerprint => Err(ControlFailure::new(
            ErrorKind::BadRequest,
            format!(
                "workflow `{workflow}` on worker {worker} has a different definition fingerprint; \
                 refresh fleet_workers before dispatching"
            ),
        )),
        Some(_) => Ok(()),
    }
}

/// Read one task, optionally waiting for it to settle first.
pub(super) async fn task_get(
    registry: &TaskRegistry,
    token: &str,
    params: &Value,
) -> Result<Value, ControlFailure> {
    let task_id = required_str(params, "taskId")?;
    let wait = params
        .get("waitSeconds")
        .and_then(Value::as_u64)
        .map(Duration::from_secs)
        .unwrap_or_default()
        .min(MAX_WAIT);
    registry
        .wait(token, &task_id, wait)
        .await
        .as_ref()
        .map(task_json)
        .ok_or_else(|| {
            ControlFailure::new(
                ErrorKind::NoSuchTask,
                format!(
                    "no task {task_id} was dispatched from this session; \
                     tasks do not survive the Medulla instance that accepted them"
                ),
            )
        })
}

/// Stop a task this grant dispatched.
pub(super) fn task_abort(
    ops: &Arc<dyn FleetOps>,
    registry: &TaskRegistry,
    token: &str,
    params: &Value,
) -> Result<Value, ControlFailure> {
    let task_id = required_str(params, "taskId")?;
    // Registry lookup keeps one grant from aborting another grant's task.
    let entry = registry.get(token, &task_id).ok_or_else(|| {
        ControlFailure::new(
            ErrorKind::NoSuchTask,
            format!("no task {task_id} was dispatched from this session"),
        )
    })?;
    if entry.state.is_settled() {
        return Ok(json!({
            "aborted": false,
            "taskId": task_id,
            "status": entry.state.status(),
        }));
    }
    if ops.abort(&entry.task_id) {
        Ok(json!({ "aborted": true, "taskId": task_id, "status": "aborting" }))
    } else {
        let status = registry
            .get(token, &task_id)
            .map_or("settled", |current| current.state.status());
        Ok(json!({ "aborted": false, "taskId": task_id, "status": status }))
    }
}
