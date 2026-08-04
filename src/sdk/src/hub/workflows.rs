//! The hub's cloud workflow plane: advertising this host's saved graphs to the
//! hosted orchestrator, and serving the reads it round-trips back.
//!
//! The store side is a [`WorkflowBridge`] the embedding host installs
//! ([`HubConfig::workflows`](crate::hub::HubConfig::workflows)); this module is
//! only the transport half — it turns one raw `medulla:workflow_request` frame
//! into exactly one `medulla:workflow_result`.

use std::sync::Arc;

use serde_json::Value;

use openhuman_core::openhuman::platform::socket::medulla::payloads::{
    RegisterWorkflows, WorkflowOp, WorkflowRequest, WorkflowResult,
};
// Re-exported rather than redeclared: the trait is the embedded core's, shared
// with the sibling openhuman host, and a second declaration here would be a
// second contract free to drift from the one the backend actually talks to.
pub use openhuman_core::openhuman::platform::socket::medulla::workflows::WorkflowBridge;

/// The installed store side of the workflow plane.
pub type WorkflowPlane = Arc<dyn WorkflowBridge>;

/// The advert batch this host currently publishes.
///
/// `None` when the bridge could not be read at all — the batch is then withheld
/// rather than sent empty, because the backend replaces this socket's whole
/// entry on every registration and an empty list would retract workflows that
/// are still installed.
pub(super) async fn advert_batch(bridge: &WorkflowPlane) -> Option<RegisterWorkflows> {
    let bridge = bridge.clone();
    // `list()` reads the host's store, so keep it off the socket runtime; a
    // panicking bridge surfaces as a `JoinError` here instead of unwinding
    // through the connect callback.
    tokio::task::spawn_blocking(move || RegisterWorkflows {
        workflows: bridge.list(),
        agent_id: bridge.agent_id(),
    })
    .await
    .ok()
}

/// The single reply one raw `medulla:workflow_request` frame earns.
///
/// `None` only when the frame carries no `requestId` — there is then nothing to
/// correlate an answer with, so the request really is unanswerable.
pub(super) async fn answer(raw: Value, bridge: Option<WorkflowPlane>) -> Option<WorkflowResult> {
    let request: WorkflowRequest = match serde_json::from_value(raw.clone()) {
        Ok(request) => request,
        // An unknown `op` (a newer backend) or a field of the wrong shape. The
        // frame is unreadable but the correlation id usually is not, and a
        // dropped request costs the backend the op's whole deadline.
        Err(err) => {
            let request_id = raw.get("requestId").and_then(Value::as_str)?;
            return Some(result_frame(
                request_id.to_string(),
                Err(format!("this host could not read the request: {err}")),
            ));
        }
    };
    let request_id = request.request_id.clone();
    let outcome = match bridge {
        Some(bridge) => dispatch(bridge, request).await,
        // A hub without an installed bridge advertises nothing, so this can only
        // be a request for a workflow some other socket owns — but it was
        // addressed here, and silence would still burn the deadline.
        None => Err("this host has no workflow store installed".to_string()),
    };
    Some(result_frame(request_id, outcome))
}

/// Route one decoded request to the installed bridge.
///
/// The bridge is a parameter rather than read from the hub's config here so the
/// whole dispatch table is exercisable against a real store without a socket.
async fn dispatch(bridge: WorkflowPlane, request: WorkflowRequest) -> Result<Value, String> {
    match request.op {
        WorkflowOp::Get => {
            let id = require_workflow_id(&request, "get")?;
            blocking(move || bridge.get(&id)).await
        }
        WorkflowOp::NodeKinds => {
            let kind = request.kind.clone();
            blocking(move || bridge.node_kinds(kind.as_deref())).await
        }
        WorkflowOp::Runs => {
            let id = require_workflow_id(&request, "runs")?;
            blocking(move || bridge.runs(&id)).await
        }
        WorkflowOp::Copilot => {
            let instruction = request
                .instruction
                .as_deref()
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .ok_or_else(|| "workflow copilot requires a non-empty instruction".to_string())?
                .to_string();
            copilot(bridge, instruction, request.workflow_id.clone()).await
        }
    }
}

/// Run one authoring turn on the host's own copilot.
///
/// Spawned rather than awaited in place for the same reason the reads are run
/// on a blocking thread: a turn is a whole agent session (the backend allows it
/// ten minutes), and a panic inside it must arrive here as a `JoinError` — the
/// request the backend is holding a ten-minute promise for is the most expensive
/// one in the protocol to drop.
async fn copilot(
    bridge: WorkflowPlane,
    instruction: String,
    workflow_id: Option<String>,
) -> Result<Value, String> {
    let turn = tokio::spawn(async move {
        bridge
            .copilot(&instruction, workflow_id.as_deref())
            .await
            .and_then(|outcome| {
                serde_json::to_value(outcome)
                    .map_err(|err| format!("failed to serialize the copilot outcome: {err}"))
            })
    });
    match turn.await {
        Ok(result) => result,
        Err(err) => Err(format!("the workflow copilot failed to answer: {err}")),
    }
}

/// Run a synchronous bridge read off the socket runtime.
///
/// Two properties in one call: a store that is slow to read cannot stall the
/// tokio worker the Socket.IO client's ping/pong shares, and a bridge that
/// *panics* comes back as a `JoinError` this maps to an error frame rather than
/// unwinding through the task and leaving the request unanswered.
async fn blocking<F>(read: F) -> Result<Value, String>
where
    F: FnOnce() -> Result<Value, String> + Send + 'static,
{
    match tokio::task::spawn_blocking(read).await {
        Ok(result) => result,
        Err(err) => Err(format!("the workflow store failed to answer: {err}")),
    }
}

/// The `workflowId` an op needs, or a message naming what was missing.
///
/// Blank is treated as absent: an empty id could only ever miss in the store,
/// and saying so up front beats a "not found" the reader cannot interpret.
fn require_workflow_id(request: &WorkflowRequest, op: &str) -> Result<String, String> {
    request
        .workflow_id
        .as_deref()
        .map(str::trim)
        .filter(|id| !id.is_empty())
        .map(str::to_string)
        .ok_or_else(|| format!("workflow {op} requires a workflowId"))
}

/// Project a dispatch outcome onto the wire frame.
fn result_frame(request_id: String, outcome: Result<Value, String>) -> WorkflowResult {
    match outcome {
        Ok(data) => WorkflowResult {
            request_id,
            ok: true,
            data: Some(data),
            error: None,
        },
        Err(error) => WorkflowResult {
            request_id,
            ok: false,
            data: None,
            error: Some(error),
        },
    }
}
