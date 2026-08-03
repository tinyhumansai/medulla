//! Cheap, local worker capacity discovery.
//!
//! Unlike the agent capability probe, this path never invokes a model. CPU
//! parallelism comes from the standard library, memory comes from the host's
//! native counters, and the IP uses the same best-effort interface selection as
//! worker onboarding.

#[cfg(target_os = "macos")]
use std::process::Command;

pub use types::WorkerSystemInfo;

mod types;

#[cfg(test)]
mod tests;

/// Capture this process's current CPU, memory, and primary-IP details.
pub fn capture_system_info() -> WorkerSystemInfo {
    let cpu_cores = std::thread::available_parallelism()
        .map(|count| count.get() as u32)
        .unwrap_or(1);
    let (memory_total_bytes, memory_available_bytes) = memory_bytes();
    WorkerSystemInfo {
        cpu_cores,
        memory_total_bytes,
        memory_available_bytes,
        ip_address: crate::worker_profile::primary_ipv4(),
    }
}

/// Read memory totals from the current platform without introducing a resident
/// system-monitor dependency.
fn memory_bytes() -> (Option<u64>, Option<u64>) {
    #[cfg(target_os = "linux")]
    {
        return std::fs::read_to_string("/proc/meminfo")
            .ok()
            .map(|text| parse_linux_meminfo(&text))
            .unwrap_or_default();
    }
    #[cfg(target_os = "macos")]
    {
        return macos_memory_bytes();
    }
    #[allow(unreachable_code)]
    (None, None)
}

/// Parse Linux `/proc/meminfo` values, whose unit is KiB.
#[cfg(any(target_os = "linux", test))]
fn parse_linux_meminfo(text: &str) -> (Option<u64>, Option<u64>) {
    fn value(text: &str, key: &str) -> Option<u64> {
        text.lines()
            .find(|line| line.starts_with(key))
            .and_then(|line| line.split_whitespace().nth(1))
            .and_then(|raw| raw.parse::<u64>().ok())
            .and_then(|kib| kib.checked_mul(1024))
    }
    (value(text, "MemTotal:"), value(text, "MemAvailable:"))
}

/// Read macOS total memory and estimate immediately reusable pages.
#[cfg(target_os = "macos")]
fn macos_memory_bytes() -> (Option<u64>, Option<u64>) {
    let total = command_number("sysctl", &["-n", "hw.memsize"]);
    let output = Command::new("vm_stat").output().ok();
    let available = output
        .filter(|result| result.status.success())
        .and_then(|result| parse_vm_stat(&String::from_utf8_lossy(&result.stdout)));
    (total, available)
}

/// Run a native counter command and parse its first numeric line.
#[cfg(target_os = "macos")]
fn command_number(command: &str, args: &[&str]) -> Option<u64> {
    let output = Command::new(command).args(args).output().ok()?;
    output
        .status
        .success()
        .then(|| String::from_utf8_lossy(&output.stdout))?
        .trim()
        .parse()
        .ok()
}

/// Sum free, inactive, and speculative pages from `vm_stat`.
#[cfg(any(target_os = "macos", test))]
fn parse_vm_stat(text: &str) -> Option<u64> {
    let page_size = text
        .lines()
        .next()?
        .split("page size of ")
        .nth(1)?
        .split_whitespace()
        .next()?
        .parse::<u64>()
        .ok()?;
    let pages: u64 = ["Pages free:", "Pages inactive:", "Pages speculative:"]
        .iter()
        .filter_map(|key| {
            text.lines()
                .find(|line| line.starts_with(key))
                .and_then(|line| line.split_whitespace().last())
                .map(|raw| raw.trim_end_matches('.'))
                .and_then(|raw| raw.parse::<u64>().ok())
        })
        .sum();
    pages.checked_mul(page_size)
}
