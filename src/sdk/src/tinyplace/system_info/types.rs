//! Data types reported by a worker's lightweight system-information probe.

use serde::{Deserialize, Serialize};

/// Capacity and network details a remote worker reports to its routing hub.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct WorkerSystemInfo {
    /// Logical CPU cores available to the worker process.
    pub cpu_cores: u32,
    /// Total physical memory in bytes, when the host exposes it.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub memory_total_bytes: Option<u64>,
    /// Currently available memory in bytes, when the host exposes it.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub memory_available_bytes: Option<u64>,
    /// Best-effort primary IPv4 address selected by the host.
    pub ip_address: String,
}
