//! Sampling and formatting of resource usage, at two scales.
//!
//! This module owns the Medulla process's own CPU, memory, and disk I/O, shown
//! as status-line segments; the [`device`] submodule owns whole-device CPU,
//! memory, and disk capacity, shown as a footer under the Agents rail. The two
//! are kept visually and textually distinct — process segments are bare
//! (`CPU 25%`), device lines are prefixed (`Device CPU 25%`) — so a reader is
//! never left guessing whose usage a number describes.
//!
//! Sampling is throttled at both scales so frequent ratatui redraws do not turn
//! the chrome into a source of measurable load itself. The process monitor
//! refreshes only the current PID and keeps a decaying disk-throughput peak for
//! relative bars.

use std::time::{Duration, Instant};

use medulla::config::{AppearanceConfig, ResourceDisplay};
use sysinfo::{ProcessRefreshKind, ProcessesToUpdate, System};

pub use device::{device_lines, DeviceMonitor};
pub use types::{DeviceSnapshot, ResourceSnapshot};

mod device;
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
        let read_rate = disk.read_bytes as f64 / elapsed;
        let write_rate = disk.written_bytes as f64 / elapsed;
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

fn bar(fraction: f64) -> String {
    const WIDTH: usize = 5;
    let filled = (fraction.clamp(0.0, 1.0) * WIDTH as f64).round() as usize;
    format!("{}{}", "█".repeat(filled), "░".repeat(WIDTH - filled))
}

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
