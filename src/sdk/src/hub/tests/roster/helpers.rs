//! Fixtures shared by the roster test modules.
//!
//! Every one of these builds the smallest value that still exercises the rule
//! under test, so a test reads as the rule and not as its setup.

use super::super::super::roster::HubWorker;

/// No liveness opinion — what a bridge with no presence signal reports, and
/// what most of these tests want, since they are about payload shape.
pub(super) fn no_presence() -> std::collections::HashMap<String, bool> {
    std::collections::HashMap::new()
}

/// A plain advertised worker: an id, an address, and the default harness.
pub(super) fn worker(id: &str, addr: &str) -> HubWorker {
    HubWorker {
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
