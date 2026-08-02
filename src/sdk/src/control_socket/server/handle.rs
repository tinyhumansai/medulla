//! The control protocol's request handler.
//!
//! Pure in the sense that matters for testing: it touches no socket and no
//! configuration it was not handed, so a fake [`FleetOps`] and a scratch
//! registry exercise every branch without a hub, a harness, or a filesystem.
//! The accept loop in [`super`] does nothing but frame lines, call this, and
//! write the answer back — the same split the OAuth loopback listener uses to
//! keep its request classification testable.

use std::sync::Arc;
use std::time::Duration;

use serde_json::{json, Value};
use uuid::Uuid;

use crate::hub::TaskRequest;
use crate::tinyplace::HarnessProvider;

use super::super::grants::{Grant, GrantRegistry};
use super::super::types::{ControlFailure, ErrorKind, FleetOps, Hello, PROTOCOL_VERSION};
use super::types::{SessionState, SpawnError, TaskEntry, TaskRegistry, TaskState};

/// The longest a `task.get` may block before answering "still running".
pub const MAX_WAIT: Duration = Duration::from_secs(120);

/// A successful reply.
fn ok(id: &Value, result: Value) -> Value {
    json!({ "v": PROTOCOL_VERSION, "id": id, "ok": true, "result": result })
}

/// A refusal.
fn fail(id: &Value, kind: ErrorKind, message: impl Into<String>) -> Value {
    json!({
        "v": PROTOCOL_VERSION,
        "id": id,
        "ok": false,
        "error": { "kind": kind.as_wire(), "message": message.into(), "retryable": kind.is_retryable() },
    })
}

/// A required string parameter, or the refusal to send instead.
fn required_str(params: &Value, key: &str) -> Result<String, ControlFailure> {
    params
        .get(key)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
        .ok_or_else(|| {
            ControlFailure::new(
                ErrorKind::BadRequest,
                format!("`{key}` is required and must be a non-empty string"),
            )
        })
}

/// An optional string parameter, treating blank as absent.
fn optional_str(params: &Value, key: &str) -> Option<String> {
    params
        .get(key)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
}

/// Render one task entry for the wire.
fn task_json(entry: &TaskEntry) -> Value {
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

/// Resolve the worker a dispatch should go to.
///
/// A named worker is matched against the roster by id or address, and a miss
/// answers with the candidates rather than a bare refusal — a model that has to
/// call `worker.list` to recover from a typo has spent a round trip on something
/// the error could have told it.
fn resolve_worker(
    ops: &Arc<dyn FleetOps>,
    requested: Option<&str>,
) -> Result<String, ControlFailure> {
    let Some(workers) = ops.workers() else {
        return Err(ControlFailure::new(
            ErrorKind::HubNotReady,
            "the fleet is still connecting; try again in a moment",
        ));
    };
    let Some(requested) = requested else {
        return ops.default_worker().ok_or_else(|| {
            ControlFailure::new(
                ErrorKind::NoSuchWorker,
                "this host has no default worker, so a dispatch has to name one; \
                 call worker.list to see what is available",
            )
        });
    };
    workers
        .iter()
        .find(|worker| worker.id == requested || worker.address == requested)
        .map(|worker| worker.address.clone())
        .ok_or_else(|| {
            let known = workers
                .iter()
                .map(|worker| worker.id.as_str())
                .collect::<Vec<_>>()
                .join(", ");
            ControlFailure::new(
                ErrorKind::NoSuchWorker,
                if known.is_empty() {
                    format!("no worker named {requested}, and this fleet has none at all")
                } else {
                    format!("no worker named {requested}; this fleet has: {known}")
                },
            )
        })
}

/// Build the dispatch request for an accepted `task.dispatch`.
///
/// The ids are minted here, never taken from the caller. They are capability
/// handles rather than names: `abort_id` is the key
/// [`abort_task`](crate::hub::TaskRunner::abort_task) looks up in a registry
/// shared by *every* dispatch on that runner, so a caller that chose it could
/// cancel work it does not own; and `task_id` is the worker-side dedupe key,
/// which a reused value silently collapses into one task.
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
        // A proposal-mode reviewer stays proposal-only when it delegates. The
        // value comes from the server-side grant, never caller parameters, so
        // the caller can neither widen nor forge the workflow scope.
        tool_mode: grant.tool_mode.clone(),
        workflow,
        workflow_inputs,
        // Context-free by default, which the field's own documentation calls
        // the invariant that lets two tasks run concurrently without seeing
        // each other's work. A parallel dispatch tool must preserve it.
        conversation: None,
        // One level below whoever asked. Taken from the *grant* rather than
        // from the request, so a caller cannot dispatch a child shallower than
        // itself and buy back the fan-out its own depth denied it.
        fleet_depth: grant.child_depth(),
    })
}

/// Handle one control request, returning the frame to write back.
///
/// Every op but `hello` requires an authenticated connection. Refusals carry a
/// machine-readable kind so the tool layer can turn "nothing was attempted" into
/// different advice than "this will never work".
pub async fn handle_control(
    ops: &Arc<dyn FleetOps>,
    grants: &GrantRegistry,
    registry: &TaskRegistry,
    session: &mut SessionState,
    request: &Value,
) -> Value {
    let id = request.get("id").cloned().unwrap_or(Value::Null);
    let op = request.get("op").and_then(Value::as_str).unwrap_or("");
    let params = request.get("params").cloned().unwrap_or(Value::Null);

    if op == "hello" {
        return match hello(grants, registry, ops, session, &params) {
            Ok(result) => ok(&id, result),
            Err(failure) => fail(&id, failure.kind, failure.message),
        };
    }

    let Some((token, _)) = session.authenticated.clone() else {
        return fail(
            &id,
            ErrorKind::Unauthenticated,
            "send `hello` with a valid grant before anything else",
        );
    };
    // Authentication is a live capability, not a one-time identity check. A
    // session revoke must stop connections that completed `hello` before the
    // harness ended as well as connections opened afterwards.
    let Some(grant) = grants.redeem(&token) else {
        session.authenticated = None;
        return fail(
            &id,
            ErrorKind::Unauthenticated,
            "that grant is no longer valid; the session it was minted for has ended",
        );
    };

    let result = match op {
        "worker.list" => worker_list(ops),
        "task.dispatch" => task_dispatch(ops, registry, &token, &grant, &params).await,
        "task.get" => task_get(registry, &token, &params).await,
        "task.list" => Ok(json!({ "tasks": registry
            .list(&token)
            .iter()
            .map(task_json)
            .collect::<Vec<_>>() })),
        "task.abort" => task_abort(ops, registry, &token, &params),
        other => Err(ControlFailure::new(
            ErrorKind::BadRequest,
            format!("unknown op: {other}"),
        )),
    };

    match result {
        Ok(result) => ok(&id, result),
        Err(failure) => fail(&id, failure.kind, failure.message),
    }
}

/// Redeem a grant and describe the fleet it reaches.
fn hello(
    grants: &GrantRegistry,
    registry: &TaskRegistry,
    ops: &Arc<dyn FleetOps>,
    session: &mut SessionState,
    params: &Value,
) -> Result<Value, ControlFailure> {
    let protocol = params
        .get("protocol")
        .and_then(Value::as_u64)
        .unwrap_or(PROTOCOL_VERSION as u64);
    if protocol != PROTOCOL_VERSION as u64 {
        return Err(ControlFailure::new(
            ErrorKind::VersionMismatch,
            format!(
                "this Medulla speaks control protocol {PROTOCOL_VERSION}, not {protocol}; \
                 a process from a different build is still running"
            ),
        ));
    }
    let token = required_str(params, "token")?;
    let grant = grants.redeem(&token).ok_or_else(|| {
        ControlFailure::new(
            ErrorKind::Unauthenticated,
            "that grant is not valid; the session it was minted for has ended",
        )
    })?;

    let hello = Hello {
        protocol: PROTOCOL_VERSION,
        version: env!("CARGO_PKG_VERSION").to_string(),
        hub_ready: ops.workers().is_some(),
        depth: grant.depth,
        max_depth: grant.max_depth,
        max_in_flight: grant.max_in_flight,
        families: grant.families,
    };
    let in_flight = registry.in_flight(&token);
    session.authenticated = Some((token, grant));
    let mut value = serde_json::to_value(&hello).unwrap_or(Value::Null);
    if let Some(object) = value.as_object_mut() {
        object.insert("inFlight".into(), json!(in_flight));
    }
    Ok(value)
}

/// The roster, or the reason it is not yet knowable.
fn worker_list(ops: &Arc<dyn FleetOps>) -> Result<Value, ControlFailure> {
    let workers = ops.workers().ok_or_else(|| {
        ControlFailure::new(
            ErrorKind::HubNotReady,
            "the fleet is still connecting; try again in a moment",
        )
    })?;
    Ok(json!({
        "workers": workers,
        "defaultWorker": ops.default_worker(),
    }))
}

/// Accept a dispatch and hand back its handle.
async fn task_dispatch(
    ops: &Arc<dyn FleetOps>,
    registry: &TaskRegistry,
    token: &str,
    grant: &Grant,
    params: &Value,
) -> Result<Value, ControlFailure> {
    // Checked here as well as withheld from the tool list. The withheld verb is
    // the guard a confused model never sees past; this is the backstop for one
    // that calls a tool it was not offered.
    //
    // The two reasons a grant may not dispatch are reported apart because they
    // mean different things to the caller: no fleet family is a decision the
    // operator made about this whole session, where the depth ceiling is about
    // where this particular harness sits.
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

/// Read one task, optionally waiting for it to settle first.
async fn task_get(
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
fn task_abort(
    ops: &Arc<dyn FleetOps>,
    registry: &TaskRegistry,
    token: &str,
    params: &Value,
) -> Result<Value, ControlFailure> {
    let task_id = required_str(params, "taskId")?;
    // Resolved through the registry so a caller can only abort ids minted for
    // its own grant; passing the requested id straight to the runner would let
    // any holder cancel any dispatch on the machine.
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
