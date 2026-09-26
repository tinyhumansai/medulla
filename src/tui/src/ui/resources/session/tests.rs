//! Formatting and sampling tests for per-session resource readings.
//!
//! The formatting cases inject a fixed [`SessionSnapshot`], so their assertions
//! are exact. The sampling cases run against this test process's own tree,
//! where the only facts that hold on every machine are structural — a reading
//! exists, it covers more than the root alone, and it is bounded.

use std::collections::HashMap;

use super::{session_segments, session_usage, SessionMonitor, SessionSnapshot};

/// A session using a quarter of a 32 GiB machine's CPU and 1.5 GiB of its RAM.
fn sample() -> SessionSnapshot {
    SessionSnapshot {
        cpu_fraction: 0.25,
        memory_bytes: 1536 * 1024 * 1024,
        total_memory_bytes: 32 * 1024 * 1024 * 1024,
        disk_bytes_per_second: 4096.0,
        processes: 3,
    }
}

/// CPU, memory and I/O in that fixed order, whatever the numbers are.
#[test]
fn segments_read_cpu_then_ram_then_io() {
    assert_eq!(
        session_segments(sample()),
        vec!["CPU 25%", "RAM 1.5G", "IO 4.0K/s"]
    );
}

/// An idle session still renders all three segments, so a title's shape does
/// not change under the operator as the harness goes quiet.
#[test]
fn an_idle_session_still_renders_every_segment() {
    assert_eq!(
        session_usage(SessionSnapshot::default()),
        "CPU 0% · RAM 0B · IO 0B/s"
    );
}

/// Injected readings answer by root pid and never touch the host.
#[test]
fn injected_readings_are_returned_for_their_pid_only() {
    let mut monitor = SessionMonitor::default();
    monitor.inject(HashMap::from([(42, sample())]));
    assert_eq!(monitor.sample(42), Some(sample()));
    assert_eq!(monitor.sample(43), None);
}

/// A pid nothing is running under has no reading, rather than a zeroed one: a
/// title must be able to tell "this session costs nothing" apart from "this
/// session is gone".
#[test]
fn a_dead_pid_has_no_reading() {
    let mut monitor = SessionMonitor::default();
    assert_eq!(monitor.sample(u32::MAX), None);
}

/// A live tree reports itself, and its readings stay inside their bounds.
#[test]
fn a_live_process_reports_a_bounded_reading() {
    let mut monitor = SessionMonitor::default();
    let pid = std::process::id();
    let sample = monitor.sample(pid).expect("this process is running");
    assert!(sample.processes >= 1, "the root itself must be counted");
    assert!((0.0..=1.0).contains(&sample.cpu_fraction));
    assert!(sample.memory_bytes > 0, "a live process holds memory");
    // The first refresh has no earlier one to measure a delta against, so the
    // accumulated I/O of a process's whole life must not be reported as a rate.
    assert_eq!(sample.disk_bytes_per_second, 0.0);
}

/// A second ask inside the throttle window is served from the memo rather than
/// re-walking the machine's process table.
#[test]
fn repeated_samples_are_stable_within_the_throttle_window() {
    let mut monitor = SessionMonitor::default();
    let pid = std::process::id();
    let first = monitor.sample(pid).expect("this process is running");
    assert_eq!(monitor.sample(pid), Some(first));
}
