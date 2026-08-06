//! The control protocol's request router and authentication boundary.
//!
//! The router authenticates each request, then delegates worker discovery and
//! task lifecycle operations to focused submodules. The accept loop in
//! [`super`] only frames lines and writes the returned response.

mod hooks;
mod params;
mod runs;
mod tasks;
mod workers;

use std::sync::Arc;

use serde_json::{json, Value};

use super::super::grants::{Grant, GrantRegistry};
use super::super::runs::HarnessRunRegistry;
use super::super::types::{ControlFailure, ErrorKind, FleetOps, Hello, PROTOCOL_VERSION};
use super::types::{SessionState, TaskRegistry};

use params::required_str;
pub use tasks::MAX_WAIT;

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

/// Handle one control request, returning the frame to write back.
///
/// Every op but `hello` requires an authenticated connection. Refusals carry a
/// machine-readable kind so the tool layer can turn "nothing was attempted" into
/// different advice than "this will never work".
pub async fn handle_control(
    ops: &Arc<dyn FleetOps>,
    grants: &GrantRegistry,
    registry: &TaskRegistry,
    runs: &HarnessRunRegistry,
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

    // A hook-only grant reaches this process's environment directly (see
    // `Grant::hook_only`'s docs) rather than the fleet grant's owner-only MCP
    // config file, so it must not be able to do anything but attribute a
    // report to its own session — every other op is refused here, before it
    // ever reaches a handler that would otherwise treat any redeemed grant as
    // license to act.
    if grant.hook_only && op != "hook.report" {
        return fail(
            &id,
            ErrorKind::Unauthenticated,
            "this grant may only file hook reports",
        );
    }

    let result = match op {
        "grant.child" => grant_child(grants, &grant),
        "worker.list" => workers::worker_list(ops).await,
        "task.dispatch" => tasks::task_dispatch(ops, registry, &token, &grant, &params).await,
        "task.get" => tasks::task_get(registry, &token, &params).await,
        "task.list" => Ok(json!({ "tasks": registry
            .list(&token)
            .iter()
            .map(tasks::task_json)
            .collect::<Vec<_>>() })),
        "task.abort" => tasks::task_abort(ops, registry, &token, &params),
        // `self::` because the module and the registry argument share a name.
        "run.report" => self::runs::run_report(runs, &grant, &params),
        "hook.report" => hooks::hook_report(ops, &grant, &params),
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

/// Mint a child capability whose authority can only narrow with depth.
fn grant_child(grants: &GrantRegistry, parent: &Grant) -> Result<Value, ControlFailure> {
    let child = Grant::new(
        parent.session.clone(),
        parent.child_depth(),
        parent.max_depth,
    )
    .with_families(parent.families)
    .with_max_in_flight(parent.max_in_flight)
    .with_tool_mode(parent.tool_mode.as_deref());
    let depth = child.depth;
    let token = grants.mint(child);
    Ok(json!({ "token": token, "depth": depth }))
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
