//! Stateful process sampler and the immutable readings passed from the
//! samplers to formatting.

use std::time::{Duration, Instant};

use sysinfo::{ProcessRefreshKind, ProcessesToUpdate, System};

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
    system: System,
    pid: Option<sysinfo::Pid>,
    last_refresh: Option<Instant>,
    disk_peak_bytes_per_second: f64,
    snapshot: ResourceSnapshot,
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

impl ResourceMonitor {
    /// Return a recent sample, refreshing at most once per second.
    pub fn sample(&mut self) -> ResourceSnapshot {
        let now = Instant::now();
        if self
            .last_refresh
            .is_some_and(|last| now.duration_since(last) < Duration::from_secs(1))
        {
            return self.snapshot;
        }
        let baseline_ready = self.last_refresh.is_some();
        let elapsed = self
            .last_refresh
            .map(|last| now.duration_since(last).as_secs_f64())
            .unwrap_or(1.0)
            .max(0.001);
        self.last_refresh = Some(now);
        self.system.refresh_memory();
        let Some(pid) = self.pid else {
            return self.snapshot;
        };
        self.system.refresh_processes_specifics(
            ProcessesToUpdate::Some(&[pid]),
            true,
            ProcessRefreshKind::nothing()
                .with_cpu()
                .with_memory()
                .with_disk_usage()
                .without_tasks(),
        );
        let Some(process) = self.system.process(pid) else {
            return self.snapshot;
        };
        let cores = std::thread::available_parallelism()
            .map(|count| count.get() as f64)
            .unwrap_or(1.0);
        let disk = process.disk_usage();
        let (read_rate, write_rate) =
            super::disk_rates(disk.read_bytes, disk.written_bytes, elapsed, baseline_ready);
        self.disk_peak_bytes_per_second = (self.disk_peak_bytes_per_second * 0.9)
            .max(read_rate)
            .max(write_rate)
            .max(1.0);
        self.snapshot = ResourceSnapshot {
            cpu_fraction: (process.cpu_usage() as f64 / (cores * 100.0)).clamp(0.0, 1.0),
            memory_bytes: process.memory(),
            total_memory_bytes: self.system.total_memory(),
            disk_read_bytes_per_second: read_rate,
            disk_write_bytes_per_second: write_rate,
            disk_peak_bytes_per_second: self.disk_peak_bytes_per_second,
        };
        self.snapshot
    }
}
