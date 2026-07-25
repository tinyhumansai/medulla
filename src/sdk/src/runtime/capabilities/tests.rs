//! Contract tests for narrow runtime capability adapters.

use super::{FleetCapability, MemoryCapability, UsageCapability};
use crate::runtime::mock::MockRuntime;

/// Read workers through only the fleet capability bound.
fn worker_count(runtime: &(impl FleetCapability + ?Sized)) -> usize {
    runtime.workers().len()
}

#[test]
fn blanket_adapter_exposes_narrow_capabilities() {
    let runtime = MockRuntime::empty();

    assert_eq!(worker_count(&runtime), 0);
    assert!(MemoryCapability::memory_status(&runtime).is_none());
    assert!(
        futures::executor::block_on(UsageCapability::team_usage(&runtime))
            .unwrap()
            .is_none()
    );
}
