//! Sampling of the Medulla process's own CPU, memory, and disk I/O.
//!
//! Kept apart from [`types`](super::types) so that file declares shape while
//! this one owns host refreshes, the throttle, CPU normalization, and the
//! decaying disk-throughput peak.

use std::time::{Duration, Instant};

use sysinfo::{ProcessRefreshKind, ProcessesToUpdate};

use super::types::{ResourceMonitor, ResourceSnapshot};

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
