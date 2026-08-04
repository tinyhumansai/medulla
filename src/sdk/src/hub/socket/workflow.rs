//! The cloud workflow plane carried over the harness socket: advertising this
//! host's saved graphs, and answering the reads the orchestrator round-trips
//! back.
//!
//! The invariant this file exists to hold is that **every request is answered**.
//! The backend holds a waiter for ten seconds on a read and ten minutes on an
//! authoring turn, so a frame dropped here is far more expensive than one
//! refused. See [`super::super::workflows`] for the store half.

use rust_socketio::asynchronous::Client;
use rust_socketio::Payload;
use serde_json::Value;

use openhuman_core::openhuman::platform::socket::medulla::payloads::{
    EVENT_REGISTER_WORKFLOWS, EVENT_WORKFLOW_RESULT,
};

use super::super::types::HubLog;
use super::super::workflows::{self, WorkflowPlane};
use super::first_obj;

/// Emit `medulla:register_workflows` for the installed bridge.
///
/// Withholds the frame entirely when the bridge could not be read (see
/// [`workflows::advert_batch`]): the backend replaces this socket's whole entry
/// on each registration, so an empty batch sent by mistake would retract graphs
/// that are still installed.
pub(super) async fn advertise_workflows(socket: &Client, bridge: &WorkflowPlane, log: &HubLog) {
    let Some(batch) = workflows::advert_batch(bridge).await else {
        log("hub: workflow store could not be read — advertising no workflows");
        return;
    };
    let count = batch.workflows.len();
    let Ok(payload) = serde_json::to_value(batch) else {
        log("hub: workflow adverts could not be serialized — advertising none");
        return;
    };
    log(&format!("hub: advertising {count} workflow(s)"));
    let _ = socket.emit(EVENT_REGISTER_WORKFLOWS, payload).await;
}

/// Answer one `medulla:workflow_request` with exactly one `workflow_result`.
///
/// The only frame this can leave unanswered is one carrying no `requestId` at
/// all — there is then nothing to correlate a reply with. Every other outcome,
/// including a bridge that panics, comes back as a result frame; see
/// [`workflows::answer`].
pub(super) async fn handle_workflow_request(
    payload: Payload,
    socket: Client,
    bridge: Option<WorkflowPlane>,
    log: HubLog,
) {
    let Some(raw) = first_obj(payload) else {
        log("hub: workflow_request carried no payload — cannot answer");
        return;
    };
    // Read before the frame is consumed, to decide whether the store may have
    // changed underneath the adverts.
    let was_copilot = raw.get("op").and_then(Value::as_str) == Some("copilot");
    let Some(result) = workflows::answer(raw, bridge.clone()).await else {
        log("hub: workflow_request carried no requestId — cannot answer");
        return;
    };
    match &result.error {
        Some(error) => log(&format!(
            "hub: workflow request {} failed — {error}",
            result.request_id
        )),
        None => log(&format!("hub: workflow request {} ok", result.request_id)),
    }
    let ok = result.ok;
    match serde_json::to_value(result) {
        Ok(payload) => {
            let _ = socket.emit(EVENT_WORKFLOW_RESULT, payload).await;
        }
        // Unreachable in practice (the frame is strings and opaque JSON), but a
        // silent return here would be the one drop this module exists to avoid.
        Err(err) => log(&format!(
            "hub: workflow result could not be serialized: {err}"
        )),
    }
    // An authoring turn is the one op that changes what this host holds, and the
    // hub has no other signal that the store moved — so the adverts are refreshed
    // here rather than left stale until the next reconnect.
    if was_copilot && ok {
        if let Some(bridge) = &bridge {
            advertise_workflows(&socket, bridge, &log).await;
        }
    }
}
