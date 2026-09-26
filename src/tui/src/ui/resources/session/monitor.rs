//! Live sampling of one harness session's process tree.

use std::collections::HashMap;
use std::time::{Duration, Instant};

use sysinfo::{Pid, ProcessRefreshKind, ProcessesToUpdate};

use super::types::{SessionMonitor, SessionSnapshot};

/// How long a set of readings is reused before the host is polled again.
///
/// Matched to the device monitor's cadence, and for the same two reasons: it is
/// unmeasurable next to a 90ms redraw loop, and it clears sysinfo's minimum CPU
/// update interval by an order of magnitude, so consecutive CPU readings are
/// real deltas rather than being clamped to zero.
///
/// Unlike the device monitor this refresh enumerates *every* process on the
/// machine — a session's cost is the cost of children it forked after it
/// started, so there is no fixed pid set to ask about — which is why the
/// interval is not shortened to make the title feel livelier.
const REFRESH_INTERVAL: Duration = Duration::from_secs(2);

impl SessionMonitor {
    /// The current reading for the session rooted at `pid`.
    ///
    /// `None` when the process is gone, which is the honest answer for a
    /// harness that exited between one frame and the next: a title showing its
    /// last known CPU would keep a dead session looking busy.
    pub fn sample(&mut self, pid: u32) -> Option<SessionSnapshot> {
        if let Some(injected) = &self.injected {
            return injected.get(&pid).copied();
        }
        self.refresh();
        if let Some(cached) = self.cached.get(&pid) {
            return Some(*cached);
        }
        let snapshot = self.walk(pid)?;
        self.cached.insert(pid, snapshot);
        Some(snapshot)
    }

    /// Poll the host, at most once per [`REFRESH_INTERVAL`].
    ///
    /// Refreshing invalidates every memoized reading: they describe the
    /// interval that just ended, and mixing them with readings from the next
    /// one would show two sessions' usage measured over different windows.
    fn refresh(&mut self) {
        let now = Instant::now();
        if self
            .last_refresh
            .is_some_and(|last| now.duration_since(last) < REFRESH_INTERVAL)
        {
            return;
        }
        self.baseline_ready = self.last_refresh.is_some();
        self.elapsed = self
            .last_refresh
            .map(|last| now.duration_since(last).as_secs_f64())
            .unwrap_or(1.0)
            .max(0.001);
        self.last_refresh = Some(now);
        self.cached.clear();
        self.system.refresh_memory();
        // `without_tasks` because on Linux a thread is enumerated as a process
        // whose parent is the process it belongs to: counting those would
        // charge a session once per thread for memory it holds once, and the
        // RAM figure would grow with a harness's thread pool.
        self.system.refresh_processes_specifics(
            ProcessesToUpdate::All,
            true,
            ProcessRefreshKind::nothing()
                .with_cpu()
                .with_memory()
                .with_disk_usage()
                .without_tasks(),
        );
    }

    /// Sum the current readings over the tree rooted at `pid`.
    fn walk(&self, pid: u32) -> Option<SessionSnapshot> {
        let root = Pid::from_u32(pid);
        self.system.process(root)?;
        // One children index for the walk rather than a parent lookup per
        // candidate: the tree is a handful of processes and the machine has
        // thousands, so scanning the machine once per level would be quadratic
        // in the wrong direction.
        let mut children: HashMap<Pid, Vec<Pid>> = HashMap::new();
        for (id, process) in self.system.processes() {
            if let Some(parent) = process.parent() {
                children.entry(parent).or_default().push(*id);
            }
        }
        let cores = std::thread::available_parallelism()
            .map(|count| count.get() as f64)
            .unwrap_or(1.0);
        let mut snapshot = SessionSnapshot {
            total_memory_bytes: self.system.total_memory(),
            ..SessionSnapshot::default()
        };
        let mut cpu_percent = 0.0f64;
        let mut disk_bytes = 0u64;
        let mut queue = vec![root];
        // A pid can only be reached once from a root, because a process has one
        // parent — so the walk needs no seen-set, only a guard against a
        // pathological cycle, which `visited` below supplies for free.
        let mut visited = std::collections::HashSet::new();
        while let Some(current) = queue.pop() {
            if !visited.insert(current) {
                continue;
            }
            let Some(process) = self.system.process(current) else {
                continue;
            };
            snapshot.processes += 1;
            cpu_percent += f64::from(process.cpu_usage());
            snapshot.memory_bytes = snapshot.memory_bytes.saturating_add(process.memory());
            let disk = process.disk_usage();
            disk_bytes = disk_bytes
                .saturating_add(disk.read_bytes)
                .saturating_add(disk.written_bytes);
            if let Some(kids) = children.get(&current) {
                queue.extend(kids.iter().copied());
            }
        }
        snapshot.cpu_fraction = (cpu_percent / (cores * 100.0)).clamp(0.0, 1.0);
        snapshot.disk_bytes_per_second = if self.baseline_ready {
            disk_bytes as f64 / self.elapsed
        } else {
            0.0
        };
        Some(snapshot)
    }

    /// Replace live sampling with fixed readings keyed by root pid, for tests.
    pub fn inject(&mut self, readings: HashMap<u32, SessionSnapshot>) {
        self.injected = Some(readings);
    }
}
