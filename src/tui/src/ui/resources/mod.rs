//! Sampling and formatting of the Medulla process's CPU, memory, and disk I/O.
//!
//! Sampling is throttled so frequent ratatui redraws do not turn the status line
//! into a source of measurable load itself. The monitor refreshes only the
//! current PID and keeps a decaying disk-throughput peak for relative bars.

use std::time::{Duration, Instant};

use medulla::config::{AppearanceConfig, ResourceDisplay};
use sysinfo::{ProcessRefreshKind, ProcessesToUpdate, System};

pub use types::ResourceSnapshot;

mod types;

#[cfg(test)]
mod tests;

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
            disk_rates(disk.read_bytes, disk.written_bytes, elapsed, baseline_ready);
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

/// Convert sysinfo's deltas to rates only after an initial counter baseline.
///
/// A process's first refresh can report accumulated startup I/O as its delta.
/// Ignoring that reading prevents it from inflating the decaying peak used by
/// subsequent disk bars.
fn disk_rates(
    read_bytes: u64,
    written_bytes: u64,
    elapsed: f64,
    baseline_ready: bool,
) -> (f64, f64) {
    if !baseline_ready {
        return (0.0, 0.0);
    }
    (read_bytes as f64 / elapsed, written_bytes as f64 / elapsed)
}

/// Format every enabled resource as compact status-line segments.
pub fn segments(config: &AppearanceConfig, sample: ResourceSnapshot) -> Vec<String> {
    let mut output = Vec::new();
    if let Some(cpu) = cpu_segment(config.cpu, sample.cpu_fraction) {
        output.push(cpu);
    }
    if let Some(ram) = ram_segment(config.ram, sample.memory_bytes, sample.total_memory_bytes) {
        output.push(ram);
    }
    if let Some(disk) = disk_segment(config.disk_io, sample) {
        output.push(disk);
    }
    output
}

fn cpu_segment(display: ResourceDisplay, fraction: f64) -> Option<String> {
    match display {
        ResourceDisplay::Off => None,
        ResourceDisplay::Percent | ResourceDisplay::Value => {
            Some(format!("CPU {:.0}%", fraction * 100.0))
        }
        ResourceDisplay::Bar => Some(format!("CPU {} {:.0}%", bar(fraction), fraction * 100.0)),
    }
}

/// Formats process RSS relative to the machine's total physical memory.
///
/// Using total memory makes the percentage meaningful alongside the absolute
/// RSS value. A platform that reports no total memory renders `0%` instead of
/// hiding an explicitly enabled metric or dividing by zero.
fn ram_segment(display: ResourceDisplay, used: u64, total: u64) -> Option<String> {
    let fraction = if total == 0 {
        0.0
    } else {
        used as f64 / total as f64
    };
    match display {
        ResourceDisplay::Off => None,
        ResourceDisplay::Percent => Some(format!("RAM {:.0}%", fraction * 100.0)),
        ResourceDisplay::Value => Some(format!("RAM {}", bytes(used as f64))),
        ResourceDisplay::Bar => Some(format!("RAM {} {:.0}%", bar(fraction), fraction * 100.0)),
    }
}

/// Formats disk throughput relative to its recent decaying peak.
///
/// The busier read/write direction determines the bar fill. Flooring the peak
/// at one byte per second avoids division by zero before any disk activity.
fn disk_segment(display: ResourceDisplay, sample: ResourceSnapshot) -> Option<String> {
    let peak = sample.disk_peak_bytes_per_second.max(1.0);
    let busiest = sample
        .disk_read_bytes_per_second
        .max(sample.disk_write_bytes_per_second);
    let value = format!(
        "R {}/s W {}/s",
        bytes(sample.disk_read_bytes_per_second),
        bytes(sample.disk_write_bytes_per_second)
    );
    match display {
        ResourceDisplay::Off => None,
        ResourceDisplay::Percent => Some(format!("IO {:.0}% peak", busiest / peak * 100.0)),
        ResourceDisplay::Value => Some(format!("IO {value}")),
        ResourceDisplay::Bar => Some(format!("IO {} {value}", bar(busiest / peak))),
    }
}

/// Renders a compact five-cell utilization bar for the one-line status area.
///
/// Five cells balance useful resolution with scarce horizontal space. Values
/// are clamped to protect the fixed width, then rounded so the closest visual
/// level is shown instead of systematically understating utilization.
fn bar(fraction: f64) -> String {
    const WIDTH: usize = 5;
    let filled = (fraction.clamp(0.0, 1.0) * WIDTH as f64).round() as usize;
    format!("{}{}", "█".repeat(filled), "░".repeat(WIDTH - filled))
}

/// Formats byte quantities compactly for the single-line status display.
///
/// Binary thresholds match memory and I/O accounting, while short `K`, `M`,
/// and `G` suffixes conserve columns. One decimal retains useful precision
/// without making rapidly changing resource values visually noisy.
fn bytes(value: f64) -> String {
    const KIB: f64 = 1024.0;
    const MIB: f64 = KIB * 1024.0;
    const GIB: f64 = MIB * 1024.0;
    if value >= GIB {
        format!("{:.1}G", value / GIB)
    } else if value >= MIB {
        format!("{:.1}M", value / MIB)
    } else if value >= KIB {
        format!("{:.1}K", value / KIB)
    } else {
        format!("{value:.0}B")
    }
}
