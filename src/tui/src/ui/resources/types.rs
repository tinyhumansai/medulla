//! Data types for resource sampling: the immutable readings passed from the
//! samplers to formatting, and the sampler's own state.
//!
//! The behaviour-heavy sampling lives in [`process_monitor`](super::process_monitor)
//! for the process scale and [`device`](super::device) for the device scale;
//! only shape and construction are declared here.

use std::time::Instant;

use sysinfo::System;

/// One current-process resource sample.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct ResourceSnapshot {
    /// CPU use as a fraction of the whole machine's logical CPU capacity.
    pub cpu_fraction: f64,
    /// Resident memory used by this process.
    pub memory_bytes: u64,
    /// Physical memory installed on the machine.
    pub total_memory_bytes: u64,
    /// Bytes this process read per second over the latest sample interval.
    pub disk_read_bytes_per_second: f64,
    /// Bytes this process wrote per second over the latest sample interval.
    pub disk_write_bytes_per_second: f64,
    /// Decaying recent peak used to scale the disk I/O bar.
    pub disk_peak_bytes_per_second: f64,
}

/// One whole-device resource sample.
///
/// Every field is optional because platform support and permissions vary. A
/// missing reading is rendered as unavailable without suppressing the others.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct DeviceSnapshot {
    /// CPU use as a fraction of the device's logical CPU capacity.
    pub cpu_fraction: Option<f64>,
    /// Physical memory currently in use, in bytes.
    pub memory_used_bytes: Option<u64>,
    /// Physical memory installed on the device, in bytes.
    pub memory_total_bytes: Option<u64>,
    /// Disk capacity currently in use on the working filesystem, in bytes.
    pub disk_used_bytes: Option<u64>,
    /// Total capacity of the working filesystem, in bytes.
    pub disk_total_bytes: Option<u64>,
}

/// Stateful, low-overhead sampler for the current Medulla process.
pub struct ResourceMonitor {
    /// sysinfo handle reused across refreshes so counters keep their baseline.
    pub(super) system: System,
    /// This process's PID, absent when the platform will not report it.
    pub(super) pid: Option<sysinfo::Pid>,
    /// When the last host refresh happened, for throttling.
    pub(super) last_refresh: Option<Instant>,
    /// Decaying peak throughput, the scale relative disk bars are drawn against.
    pub(super) disk_peak_bytes_per_second: f64,
    /// Most recent reading, handed back while inside the throttle interval.
    pub(super) snapshot: ResourceSnapshot,
}

impl Default for ResourceMonitor {
    fn default() -> Self {
        Self {
            system: System::new(),
            pid: sysinfo::get_current_pid().ok(),
            last_refresh: None,
            disk_peak_bytes_per_second: 1.0,
            snapshot: ResourceSnapshot::default(),
        }
    }
}
