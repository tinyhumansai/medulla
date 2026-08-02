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

use medulla::config::{AppearanceConfig, ResourceDisplay};

pub use device::{device_lines, DeviceMonitor};
pub use types::{DeviceSnapshot, ResourceMonitor, ResourceSnapshot};

mod device;
mod process_monitor;
mod types;

#[cfg(test)]
mod tests;

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

/// Format CPU usage as a status-line segment.
///
/// `Percent` and `Value` render identically as a percentage: CPU usage is
/// inherently normalized by the system and the percentage is already the
/// complete picture. `Bar` adds a full-machine capacity visualization: the bar
/// shows usage relative to available cores, scaled by the process count, so a
/// 4-core system at full CPU shows a 100% bar even when the process uses only
/// 25%.
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
