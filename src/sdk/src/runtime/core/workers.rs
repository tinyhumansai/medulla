//! Parsing of the core's `worker.list`-shaped payloads into the runtime's
//! [`WorkerInfo`] rows for the Agents/fleet surface.

use serde_json::Value;

use crate::runtime::WorkerInfo;

use super::events::opt_str;

/// Parse a `worker.list`-shaped payload `{workers: [...], selectedId}` into rows.
pub(super) fn workers_from_payload(payload: &Value) -> Vec<WorkerInfo> {
    payload
        .get("workers")
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(|w| {
                    let id = w.get("id").and_then(Value::as_str)?.to_string();
                    let address = w.get("address").and_then(Value::as_str)?.to_string();
                    Some(WorkerInfo {
                        id,
                        address,
                        handle: opt_str(w, "handle"),
                        label: opt_str(w, "label"),
                        harness: opt_str(w, "harness"),
                        peer_id: opt_str(w, "peerId"),
                        selected: w.get("selected").and_then(Value::as_bool).unwrap_or(false),
                    })
                })
                .collect()
        })
        .unwrap_or_default()
}
