//! Data types for per-session resource sampling: the reading a title is drawn
//! from, and the sampler's own state.
//!
//! Shape only. The host refreshes, the process-tree walk, and the throttle live
//! in [`monitor`](super::monitor); the rendering lives in
//! [`format`](super::format).

use std::collections::HashMap;
use std::time::Instant;

use sysinfo::System;

/// What one harness session is costing the machine right now.
///
/// A *tree* reading, not a single process's: a harness CLI spends most of its
/// life with the compiler, test runner, or `rg` it just forked doing the actual
/// work, so charging the session only for its own root process would report
/// near-zero for exactly the sessions an operator is watching because they are
/// expensive.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct SessionSnapshot {
    /// CPU use of the whole process tree, as a fraction of the machine's total
    /// logical CPU capacity.
    pub cpu_fraction: f64,
    /// Resident memory summed over the process tree.
    pub memory_bytes: u64,
    /// Physical memory installed on the machine, for the RAM percentage.
    pub total_memory_bytes: u64,
    /// Bytes per second the tree read and wrote over the latest interval,
    /// combined — one number, because a title has room for one.
    pub disk_bytes_per_second: f64,
    /// How many processes the reading covers, root included.
    pub processes: usize,
}

/// Stateful, low-overhead sampler for the harness sessions this device hosts.
///
/// One [`System`] serves every session: the host refresh that makes a reading
/// possible is whole-machine work, and doing it per session would multiply the
/// most expensive part of sampling by the number of panes.
pub struct SessionMonitor {
    /// sysinfo handle reused across refreshes so CPU and I/O counters keep the
    /// baseline their deltas are measured against.
    pub(super) system: System,
    /// When the last host refresh happened, for throttling.
    pub(super) last_refresh: Option<Instant>,
    /// Seconds covered by the interval the current readings describe.
    pub(super) elapsed: f64,
    /// Whether a previous refresh exists for deltas to be measured against.
    ///
    /// A process's first refresh reports everything it has read and written
    /// since it started as one delta; attributing that to a single interval
    /// would show a session that has just opened doing gigabytes a second.
    pub(super) baseline_ready: bool,
    /// Readings already computed since the last refresh, keyed by root pid.
    ///
    /// The walk is memoized rather than repeated because the render loop asks
    /// for the same session many times between two refreshes, and each ask
    /// would otherwise rebuild the machine's whole parent/child map.
    pub(super) cached: HashMap<u32, SessionSnapshot>,
    /// Test-injected readings that bypass all host sampling, keyed by root pid.
    pub(super) injected: Option<HashMap<u32, SessionSnapshot>>,
}

impl Default for SessionMonitor {
    fn default() -> Self {
        Self {
            // Processes are discovered by the first sample rather than at
            // startup: a device hosting no sessions never pays for the scan.
            system: System::new(),
            last_refresh: None,
            elapsed: 1.0,
            baseline_ready: false,
            cached: HashMap::new(),
            injected: None,
        }
    }
}
