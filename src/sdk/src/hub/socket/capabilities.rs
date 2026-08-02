//! Serving one `medulla:capabilities_request`.
//!
//! The static roster facts answer on their own, so a probe never depends on the
//! worker being reachable; the live budgets and readiness are decoration layered
//! on top when the worker does answer.

use std::sync::Arc;

use rust_socketio::asynchronous::Client;
use rust_socketio::Payload;
use serde_json::json;

use super::super::roster::SharedRoster;
use super::super::runner::TaskRunner;
use super::{first_obj, str_field};

/// Answer a capability probe, decorating the static roster facts with the
/// worker's live budgets/readiness.
///
/// The static facts (`providers`, `summary`) are established from the roster
/// without touching the worker, so a probe always answers even if the worker is
/// unreachable. On top of that, the hub asks the resolved worker for its
/// [`AgentCapabilities`](crate::tinyplace::AgentCapabilities) over tiny.place and
/// maps its `budgets`/`readiness` onto the backend-shaped keys
/// (`harnessBudgets`, `ready`, `readyReason`) that the backend's
/// `sanitizeCapabilities` reads. The probe fails open: any transport error,
/// timeout, or malformed reply simply omits those keys rather than blocking the
/// answer.
pub(super) async fn handle_capabilities(
    payload: Payload,
    socket: Client,
    roster: SharedRoster,
    catalog: Arc<Vec<crate::runtime::AgentTemplate>>,
    runner: Arc<TaskRunner>,
) {
    let Some(obj) = first_obj(payload) else {
        return;
    };
    let Some(probe_id) = str_field(&obj, "probeId") else {
        return;
    };
    let agent_id = str_field(&obj, "agentId").unwrap_or_default();
    // Resolve the targeted worker (or the selected/first when unattributed),
    // then drop the lock before any await.
    let worker = {
        let r = roster.lock().expect("roster lock");
        let wanted = agent_id.trim();
        let found = if wanted.is_empty() {
            r.iter().find(|w| w.selected).or_else(|| r.first())
        } else {
            r.iter().find(|w| w.id == wanted || w.address == wanted)
        };
        found.cloned()
    };
    let (harness, address, roles) = match &worker {
        Some(w) => (w.harness.clone(), Some(w.address.clone()), w.roles.clone()),
        None => (String::new(), None, Vec::new()),
    };
    let allowed_tools = super::super::probe::role_tool_allowlist(&roles, &catalog);
    // Ask the worker what it can actually do, and answer with that. Fails open:
    // a transport error, timeout, or malformed reply leaves only the static
    // facts the roster already knows. See [`super::super::probe`].
    let caps = match address {
        Some(address) => runner.capabilities(&address).await.ok(),
        None => None,
    };
    let capabilities = super::super::probe::capabilities_payload(
        &harness,
        caps.as_ref(),
        allowed_tools.as_deref(),
        &roles,
    );
    let _ = socket
        .emit(
            "medulla:capabilities_result",
            json!({ "probeId": probe_id, "capabilities": capabilities }),
        )
        .await;
}
