//! Host resource sampling for the Medulla process, its device, and harness sessions.
//!
//! This crate owns the sysinfo-backed samplers and the immutable snapshots they
//! produce. Presentation and configuration stay with the terminal UI. Every
//! monitor exposes [`inject`](ResourceMonitor::inject) or its corresponding
//! method so callers can provide deterministic readings in tests.

pub mod agents;
mod cpu;
mod device;
mod process_monitor;
pub mod sampler;
mod session;
mod types;

pub use cpu::{CpuField, CpuMonitor, CpuReading, CpuSnapshot};
pub use device::DeviceMonitor;
pub use sampler::{
    DiskOptions, DiskSnapshot, NetworkSnapshot, ProcessSnapshot, ResourceHistory,
    SystemResourceSource,
};
pub use session::{SessionMonitor, SessionSnapshot};
pub use types::{DeviceSnapshot, ResourceMonitor, ResourceSnapshot};

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

#[cfg(test)]
mod tests;
