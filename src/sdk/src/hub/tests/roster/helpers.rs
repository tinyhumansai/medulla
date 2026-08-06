//! Fixtures shared by the roster test modules.
//!
//! Every one of these builds the smallest value that still exercises the rule
//! under test, so a test reads as the rule and not as its setup.

use super::super::super::roster::HubWorker;
use crate::protocol::{
    BudgetSource, BudgetWindow, HarnessBudget, HarnessProvider, WorkerSystemInfo,
};

/// No liveness opinion — what a bridge with no presence signal reports, and
/// what most of these tests want, since they are about payload shape.
pub(super) fn no_presence() -> std::collections::HashMap<String, bool> {
    std::collections::HashMap::new()
}

/// A plain advertised worker: an id, an address, and the default harness.
pub(super) fn worker(id: &str, addr: &str) -> HubWorker {
    HubWorker {
        roles: Vec::new(),
        id: id.to_string(),
        address: addr.to_string(),
        harness: "claude".to_string(),
        label: None,
        selected: false,
        workspace: None,
        ..Default::default()
    }
}

/// The same shape as [`worker`], under the name the dedupe tests use.
pub(super) fn hw(id: &str, address: &str) -> HubWorker {
    worker(id, address)
}

/// Measured capacity: `cpu` cores and `available_gib` of free memory.
pub(super) fn details(cpu: u32, available_gib: u64) -> WorkerSystemInfo {
    WorkerSystemInfo {
        cpu_cores: cpu,
        memory_total_bytes: None,
        memory_available_bytes: Some(available_gib * 1024 * 1024 * 1024),
        ip_address: "10.0.0.1".into(),
    }
}

/// A weekly budget for `provider` with `remaining_tokens` left of
/// `limit_tokens`.
pub(super) fn provider_budget(
    provider: HarnessProvider,
    limit_tokens: i64,
    remaining_tokens: i64,
) -> HarnessBudget {
    HarnessBudget {
        provider,
        seat: None,
        window: BudgetWindow::Weekly,
        limit_tokens: Some(limit_tokens),
        used_tokens: Some(limit_tokens - remaining_tokens),
        remaining_tokens: Some(remaining_tokens),
        cooldown_until: None,
        source: BudgetSource::Configured,
    }
}

/// One declared local host, as `local_hosts` resolves one.
pub(super) fn declared(id: &str, name: &str) -> crate::config::LocalHostRef {
    crate::config::LocalHostRef {
        id: id.to_string(),
        name: name.to_string(),
        workspace: String::new(),
        primary: false,
    }
}

/// A worker placed on `host_id`, reached at that host's address.
pub(super) fn placed(id: &str, host_id: &str) -> HubWorker {
    HubWorker {
        host_id: host_id.to_string(),
        address: host_id.to_string(),
        ..worker(id, host_id)
    }
}
