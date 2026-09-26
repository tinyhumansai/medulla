//! Stateful data held by whole-device resource sampling.

use std::time::Instant;

use sysinfo::{Disks, System};

use crate::ui::resources::types::DeviceSnapshot;

/// Stateful, low-overhead sampler for whole-device CPU, memory, and disk use.
pub struct DeviceMonitor {
    pub(super) system: System,
    pub(super) disks: Disks,
    pub(super) last_refresh: Option<Instant>,
    pub(super) last_refresh_included_disk: bool,
    pub(super) snapshot: DeviceSnapshot,
    /// Test-injected reading that bypasses all host sampling.
    pub(super) injected: Option<DeviceSnapshot>,
}

impl Default for DeviceMonitor {
    fn default() -> Self {
        Self {
            // Host resources, including mounts, are discovered lazily by the
            // first enabled metric sample rather than during application startup.
            system: System::new(),
            disks: Disks::new(),
            last_refresh: None,
            last_refresh_included_disk: false,
            snapshot: DeviceSnapshot::default(),
            injected: None,
        }
    }
}
