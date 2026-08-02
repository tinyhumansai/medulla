//! Live sampling of whole-device CPU, memory, and disk readings.

use std::time::{Duration, Instant};

use sysinfo::Disks;

use crate::ui::resources::types::DeviceSnapshot;

use super::types::DeviceMonitor;

/// How long a sample is reused before the device is polled again.
///
/// Two seconds is slow enough that the cost is unmeasurable next to a 90ms
/// redraw loop, and fast enough that a machine tipping into memory or CPU
/// pressure is visible before an operator would have reached for `top`. It also
/// clears sysinfo's minimum CPU update interval by an order of magnitude, so
/// consecutive CPU readings are meaningful rather than clamped to zero.
const REFRESH_INTERVAL: Duration = Duration::from_secs(2);

impl DeviceMonitor {
    /// Return a recent whole-device sample, refreshing at most every
    /// [`REFRESH_INTERVAL`].
    ///
    /// The first call reports CPU as unavailable: utilization is a delta
    /// between two refreshes, and there is no earlier one to compare against.
    /// Memory and disk are absolute and are available immediately.
    pub fn sample(&mut self) -> DeviceSnapshot {
        self.sample_for(true)
    }

    /// Return a recent sample, discovering mounts only when disk capacity will
    /// actually be displayed.
    pub(crate) fn sample_for(&mut self, include_disk: bool) -> DeviceSnapshot {
        if let Some(injected) = self.injected {
            return injected;
        }
        let now = Instant::now();
        let first = self.last_refresh.is_none();
        if self.last_refresh.is_some_and(|last| {
            now.duration_since(last) < REFRESH_INTERVAL
                && (self.last_refresh_included_disk || !include_disk)
        })
        {
            return self.snapshot;
        }
        self.last_refresh = Some(now);
        self.last_refresh_included_disk = include_disk;
        self.system.refresh_cpu_usage();
        self.system.refresh_memory();
        // Rebuild the Disks instance to discover newly mounted filesystems.
        // `Disks::refresh(true)` only refreshes existing entries and does not
        // discover new mounts; `new_with_refreshed_list()` discovers the current
        // mount inventory, so we rebuild on each refresh to detect dynamic changes.
        if include_disk {
            self.disks = Disks::new_with_refreshed_list();
        }

        let memory_total = self.system.total_memory();
        let capacity = include_disk.then(|| self.working_disk()).flatten();
        self.snapshot = DeviceSnapshot {
            // A single refresh only primes the counters; reporting the value it
            // returns would show a confident 0% on the first frame.
            cpu_fraction: (!first)
                .then(|| (f64::from(self.system.global_cpu_usage()) / 100.0).clamp(0.0, 1.0)),
            memory_used_bytes: (memory_total > 0).then(|| self.system.used_memory()),
            memory_total_bytes: (memory_total > 0).then_some(memory_total),
            disk_used_bytes: capacity.map(|(total, available)| total.saturating_sub(available)),
            disk_total_bytes: capacity.map(|(total, _)| total),
        };
        self.snapshot
    }

    /// Replace live sampling with a fixed reading, for tests.
    pub fn inject(&mut self, snapshot: DeviceSnapshot) {
        self.injected = Some(snapshot);
    }

    /// Total and available bytes of the filesystem Medulla is running against.
    ///
    /// Summing every mount would be actively misleading: Linux reports bind
    /// mounts, `tmpfs`, and btrfs subvolumes as separate disks backed by the
    /// same bytes, so the total would exceed the hardware. Reporting the single
    /// filesystem holding the working directory is a number the operator can
    /// act on — it is the one that fills up when a run writes too much. The
    /// mount whose path is the longest prefix of the working directory wins,
    /// because `/home` must beat `/` for a working directory under it.
    ///
    /// Returns `None` when no mount can be attributed, which the sidebar shows
    /// as an unavailable reading rather than as zero capacity.
    fn working_disk(&self) -> Option<(u64, u64)> {
        let cwd = std::env::current_dir().ok();
        let mut best: Option<(usize, u64, u64)> = None;
        for disk in self.disks.list() {
            if disk.total_space() == 0 {
                continue;
            }
            let mount = disk.mount_point();
            // Rank by how specifically the mount contains the working
            // directory; an unreadable working directory leaves every mount
            // tied at zero, and the largest one is then the best guess.
            let depth = match &cwd {
                Some(cwd) if cwd.starts_with(mount) => mount.components().count(),
                Some(_) => continue,
                None => 0,
            };
            let better = best.is_none_or(|(best_depth, best_total, _)| {
                depth > best_depth || (depth == best_depth && disk.total_space() > best_total)
            });
            if better {
                best = Some((depth, disk.total_space(), disk.available_space()));
            }
        }
        best.map(|(_, total, available)| (total, available))
    }
}
