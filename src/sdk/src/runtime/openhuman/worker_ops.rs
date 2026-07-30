//! Adapting hub workers and worker mutations to the embedded runtime surface.
//!
//! Restored verbatim from the deleted backend runtime. The mapping is a
//! property of the hub's roster shape and the UI's `WorkerInfo`, not of whoever
//! is asking: when the backend runtime went away it took the only implementation
//! with it, which is how the Workers tab ended up empty and its mutations
//! silently successful on the embedded path.

use anyhow::anyhow;

/// Map a hub roster entry to the UI's [`WorkerInfo`](crate::runtime::WorkerInfo)
/// row. An `@handle` address is surfaced as the `handle` field too.
///
/// `pub(super)` lets the sibling runtime and tests share the exact mapping
/// without standing up a live hub handle.
pub(super) fn hub_worker_to_info(
    worker: crate::hub::HubWorker,
    details: Option<crate::tinyplace::WorkerSystemInfo>,
) -> crate::runtime::WorkerInfo {
    let (cpu_cores, memory_total_bytes, memory_available_bytes, ip_address) = details
        .map(|details| {
            (
                Some(details.cpu_cores),
                details.memory_total_bytes,
                details.memory_available_bytes,
                Some(details.ip_address),
            )
        })
        .unwrap_or_default();
    crate::runtime::WorkerInfo {
        handle: worker
            .address
            .starts_with('@')
            .then(|| worker.address.clone()),
        id: worker.id,
        workspace: worker.workspace,
        address: worker.address,
        label: worker.label,
        harness: Some(worker.harness),
        roles: worker.roles,
        peer_id: None,
        cpu_cores,
        memory_total_bytes,
        memory_available_bytes,
        ip_address,
        selected: worker.selected,
        // Budgets/readiness ride the capability probe, not the roster mapping;
        // the hub projection carries none, so they default empty here.
        budgets: Vec::new(),
        readiness: Vec::new(),
    }
}

/// Translate a [`WorkerOp`](crate::runtime::WorkerOp) into a hub-handle mutation.
pub(super) async fn apply_worker_op(
    handle: &crate::hub::HubHandle,
    op: crate::runtime::WorkerOp,
) -> anyhow::Result<()> {
    use crate::runtime::WorkerOp;
    match op {
        WorkerOp::Add {
            address,
            handle: named_handle,
            label,
            harness,
        } => {
            let address = address
                .or(named_handle)
                .ok_or_else(|| anyhow!("a worker needs an address or @handle"))?;
            handle
                .add(crate::hub::HubWorker {
                    id: address.clone(),
                    address,
                    harness: harness.unwrap_or_else(|| "claude".to_string()),
                    label,
                    roles: Vec::new(),
                    selected: false,
                    // A peer added by address: this hub has no idea where it
                    // runs tasks. The backend falls back to its probed cwd.
                    workspace: None,
                })
                .await
        }
        WorkerOp::Remove { id } => handle.remove(&id).await,
        WorkerOp::SetRoles { id, roles } => handle.set_roles(&id, roles).await,
        WorkerOp::RefreshDetails { id } => handle.refresh_system_info(&id).await,
        WorkerOp::ApplyStrategy { strategy } => handle.apply_strategy(strategy),
        WorkerOp::ApplySubscriptionStrategy { strategy } => {
            handle.apply_subscription_strategy(strategy);
            Ok(())
        }
        WorkerOp::Update { id, patch } => {
            let label = label_from_patch(&patch)?;
            handle.set_label(&id, label).await
        }
        WorkerOp::Select { id } => {
            handle.select(&id);
            Ok(())
        }
    }
}

/// Validate the only supported worker patch, preserving `""` as explicit clear.
pub(super) fn label_from_patch(
    patch: &serde_json::Map<String, serde_json::Value>,
) -> anyhow::Result<Option<String>> {
    let label = patch
        .get("label")
        .and_then(serde_json::Value::as_str)
        .ok_or_else(|| anyhow!("worker update requires a string label"))?;
    Ok((!label.is_empty()).then(|| label.to_string()))
}
