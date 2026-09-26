//! Fleet roster resolution and bounded workflow-capability discovery.

use std::sync::Arc;
use std::time::Duration;

use futures::future::join_all;
use serde_json::{json, Value};

use super::super::super::types::{ControlFailure, ErrorKind, FleetOps};

/// Keep best-effort discovery inside the control client's five-second deadline.
const WORKFLOW_PROBE_TIMEOUT: Duration = Duration::from_secs(2);

/// Return the roster with each responsive worker's workflow catalog.
pub(super) async fn worker_list(ops: &Arc<dyn FleetOps>) -> Result<Value, ControlFailure> {
    let mut workers = ops.workers().ok_or_else(|| {
        ControlFailure::new(
            ErrorKind::HubNotReady,
            "the fleet is still connecting; try again in a moment",
        )
    })?;
    let catalogs = join_all(
        workers
            .iter()
            .map(|worker| probe_worker_workflows(ops, &worker.address)),
    )
    .await;
    for (worker, catalog) in workers.iter_mut().zip(catalogs) {
        worker.workflows = catalog.unwrap_or_default();
    }
    Ok(json!({
        "workers": workers,
        "defaultWorker": ops.default_worker(),
    }))
}

/// Probe one worker without letting its retry window consume the MCP call.
pub(super) async fn probe_worker_workflows(
    ops: &Arc<dyn FleetOps>,
    worker: &str,
) -> Result<Vec<crate::protocol::WorkflowAdvert>, crate::hub::RunError> {
    tokio::time::timeout(WORKFLOW_PROBE_TIMEOUT, ops.worker_workflows(worker))
        .await
        .map_err(|_| crate::hub::RunError::Timeout)?
}

/// Resolve a requested worker id/address, or the operator's default worker.
pub(super) fn resolve_worker(
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
