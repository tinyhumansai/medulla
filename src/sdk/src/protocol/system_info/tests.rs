//! Deterministic parser and capture tests for worker system information.

use super::{capture_system_info, parse_linux_meminfo, parse_vm_stat};

#[test]
fn linux_meminfo_reports_bytes_not_kib() {
    let text = "MemTotal:       16384 kB\nMemAvailable:    4096 kB\n";
    assert_eq!(
        parse_linux_meminfo(text),
        (Some(16 * 1024 * 1024), Some(4 * 1024 * 1024))
    );
}

#[test]
fn vm_stat_sums_reusable_pages() {
    let text = "\
Mach Virtual Memory Statistics: (page size of 4096 bytes)
Pages free:                               10.
Pages active:                             50.
Pages inactive:                           20.
Pages speculative:                         5.
";
    assert_eq!(parse_vm_stat(text), Some(35 * 4096));
}

#[test]
fn capture_always_reports_cpu_and_ip() {
    let info = capture_system_info();
    assert!(info.cpu_cores >= 1);
    assert!(!info.ip_address.is_empty());
}
