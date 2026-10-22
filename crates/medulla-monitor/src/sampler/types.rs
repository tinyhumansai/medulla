//! Snapshot data returned by the background resource sampler.

/// Point-in-time disk capacity reading.
#[derive(Debug, Clone, PartialEq)]
pub struct DiskSnapshot {
    /// Device name when the platform exposes one.
    pub device: String,
    /// Mount point or volume name.
    pub mount: String,
    /// Total capacity in bytes.
    pub total_bytes: u64,
    /// Capacity in use in bytes.
    pub used_bytes: u64,
    /// Read throughput from the preceding sample, where supported.
    pub read_bytes_per_second: f64,
    /// Write throughput from the preceding sample, where supported.
    pub write_bytes_per_second: f64,
    /// Device busy percentage, unavailable on non-Linux collectors.
    pub busy_percent: Option<f64>,
}

/// Point-in-time counters and addresses for one network interface.
#[derive(Debug, Clone, PartialEq)]
pub struct NetworkSnapshot {
    /// Operating-system interface name.
    pub interface: String,
    /// Whether the interface is administratively up.
    pub up: bool,
    /// Whether the interface currently has carrier or observed traffic.
    pub connected: bool,
    /// Assigned IPv4 and IPv6 addresses.
    pub ip_addresses: Vec<String>,
    /// Received bytes per second from the preceding sample.
    pub received_bytes_per_second: f64,
    /// Transmitted bytes per second from the preceding sample.
    pub transmitted_bytes_per_second: f64,
    /// Bytes received since the interface counter was initialized.
    pub total_received_bytes: u64,
    /// Bytes transmitted since the interface counter was initialized.
    pub total_transmitted_bytes: u64,
}

/// Point-in-time process reading.
#[derive(Debug, Clone, PartialEq)]
pub struct ProcessSnapshot {
    /// Process identifier.
    pub pid: u32,
    /// Parent process identifier when available.
    pub ppid: Option<u32>,
    /// Executable path when available.
    pub exe: Option<String>,
    /// Command line arguments.
    pub cmd: Vec<String>,
    /// Current working directory when available.
    pub cwd: Option<String>,
    /// Start time as seconds since Unix epoch.
    pub start_time: u64,
    /// Effective user id when the platform exposes it.
    pub owner_uid: Option<String>,
    /// CPU utilization as a percentage.
    pub cpu_percent: f64,
    /// Resident memory in bytes.
    pub memory_bytes: u64,
    /// Bytes read since the previous system refresh.
    pub disk_read_bytes: u64,
    /// Bytes written since the previous system refresh.
    pub disk_written_bytes: u64,
    /// Number of threads/tasks reported for this process.
    pub threads: usize,
}

/// Immutable whole-device readings suitable for sharing with renderers.
#[derive(Debug, Clone, Default)]
pub struct ResourceSnapshot {
    /// Whole-machine CPU details and Linux-only hardware readings.
    pub cpu: crate::CpuSnapshot,
    /// Current process metrics used by the status line.
    pub current_process: Option<crate::ResourceSnapshot>,
    /// Recent readings accumulated by the sampler for graph rendering.
    pub history: std::sync::Arc<ResourceHistory>,
    /// Seconds since the previous sample, used to scale process I/O deltas.
    pub sample_interval_seconds: f64,
    /// Total CPU utilization as a percentage.
    pub cpu_percent: f64,
    /// Utilization percentages for each logical CPU.
    pub cpu_cores_percent: Vec<f64>,
    /// Physical memory totals and readings, in bytes.
    pub memory_total_bytes: u64,
    /// Physical memory currently in use, in bytes.
    pub memory_used_bytes: u64,
    /// Physical memory available to applications, in bytes.
    pub memory_available_bytes: u64,
    /// Cached physical memory, in bytes.
    pub memory_cached_bytes: u64,
    /// Physical memory currently unused, in bytes.
    pub memory_free_bytes: u64,
    /// Total swap capacity, in bytes.
    pub swap_total_bytes: u64,
    /// Swap in use, in bytes.
    pub swap_used_bytes: u64,
    /// Mounted disk readings.
    pub disks: Vec<DiskSnapshot>,
    /// Per-interface traffic, state, and addresses.
    pub networks: Vec<NetworkSnapshot>,
    /// Process table in the order sysinfo supplied it.
    pub processes: Vec<ProcessSnapshot>,
    /// Lookup prepared off-thread for fast session rollups during rendering.
    pub(crate) process_index: std::sync::Arc<std::collections::HashMap<u32, usize>>,
    /// Parent-to-child map prepared off-thread with the process table.
    pub(crate) children_index: std::sync::Arc<std::collections::HashMap<u32, Vec<u32>>>,
}

impl ResourceSnapshot {
    /// Attach whole-machine CPU readings to a snapshot.
    pub fn with_cpu(mut self, cpu: crate::CpuSnapshot) -> Self {
        self.cpu = cpu;
        self
    }

    /// Attach the history accumulated by a background sampler.
    pub fn with_history(mut self, history: std::sync::Arc<ResourceHistory>) -> Self {
        self.history = history;
        self
    }

    /// Adapt whole-device readings for the existing compact footer.
    pub fn device_snapshot(&self) -> crate::DeviceSnapshot {
        let cwd = std::env::current_dir().ok();
        let disk = cwd
            .as_deref()
            .and_then(|cwd| most_specific_mount(&self.disks, cwd));
        crate::DeviceSnapshot {
            cpu_fraction: Some((self.cpu_percent / 100.0).clamp(0.0, 1.0)),
            memory_used_bytes: Some(self.memory_used_bytes),
            memory_total_bytes: Some(self.memory_total_bytes),
            disk_used_bytes: disk.map(|disk| disk.used_bytes),
            disk_total_bytes: disk.map(|disk| disk.total_bytes),
        }
    }

    /// Return this snapshot with a process table and its render-time indexes.
    ///
    /// Resource sources can use this builder to publish process readings
    /// without depending on the system sampler's implementation details.
    pub fn with_processes(mut self, processes: Vec<ProcessSnapshot>) -> Self {
        let process_index = std::sync::Arc::new(
            processes
                .iter()
                .enumerate()
                .map(|(index, process)| (process.pid, index))
                .collect(),
        );
        let mut children = std::collections::HashMap::<u32, Vec<u32>>::new();
        for process in &processes {
            if let Some(parent) = process.ppid {
                children.entry(parent).or_default().push(process.pid);
            }
        }
        self.processes = processes;
        self.process_index = process_index;
        self.children_index = std::sync::Arc::new(children);
        self
    }

    /// Aggregate the cached process table for a session, without touching the
    /// host from the render path.
    pub fn session_snapshot(&self, root_pid: u32) -> Option<crate::SessionSnapshot> {
        let root = self.processes.get(*self.process_index.get(&root_pid)?)?;
        let core_count = self.cpu_cores_percent.len().max(1) as f64;
        let mut result = crate::SessionSnapshot {
            total_memory_bytes: self.memory_total_bytes,
            ..Default::default()
        };
        let mut pending = vec![root.pid];
        let mut visited = std::collections::HashSet::new();
        while let Some(pid) = pending.pop() {
            if !visited.insert(pid) {
                continue;
            }
            let Some(process) = self
                .process_index
                .get(&pid)
                .and_then(|index| self.processes.get(*index))
            else {
                continue;
            };
            result.processes += 1;
            result.cpu_fraction += process.cpu_percent / (core_count * 100.0);
            result.memory_bytes = result.memory_bytes.saturating_add(process.memory_bytes);
            result.disk_bytes_per_second += (process.disk_read_bytes + process.disk_written_bytes)
                as f64
                / self.sample_interval_seconds.max(0.001);
            if let Some(descendants) = self.children_index.get(&pid) {
                pending.extend(descendants);
            }
        }
        result.cpu_fraction = result.cpu_fraction.clamp(0.0, 1.0);
        Some(result)
    }
}

/// Pick the deepest mount containing `cwd`; ties prefer the larger capacity.
pub(super) fn most_specific_mount<'a>(
    disks: &'a [DiskSnapshot],
    cwd: &std::path::Path,
) -> Option<&'a DiskSnapshot> {
    disks
        .iter()
        .filter(|disk| cwd.starts_with(std::path::Path::new(&disk.mount)))
        .max_by_key(|disk| {
            (
                std::path::Path::new(&disk.mount).components().count(),
                disk.total_bytes,
            )
        })
}

/// Recent scalar series shared between the sampler and rendering views.
#[derive(Debug, Clone, Default)]
pub struct ResourceHistory {
    /// Whole-device CPU utilization percentages.
    pub cpu_percent: super::History<f64>,
    /// CPU field readings over time.
    pub cpu: super::History<crate::CpuSnapshot>,
    /// Physical memory currently used, in bytes.
    pub memory_used_bytes: super::History<u64>,
    /// Memory available to applications.
    pub memory_available_bytes: super::History<u64>,
    /// Cached memory.
    pub memory_cached_bytes: super::History<u64>,
    /// Unused physical memory, in bytes.
    pub memory_free_bytes: super::History<u64>,
    /// Swap currently used, in bytes.
    pub swap_used_bytes: super::History<u64>,
    /// Combined disk read throughput.
    pub disk_read_bytes_per_second: super::History<f64>,
    /// Combined disk write throughput.
    pub disk_write_bytes_per_second: super::History<f64>,
    /// Per-device read, write, and busy readings for graph rendering.
    pub disk_io: super::History<Vec<DiskSnapshot>>,
    /// Per-interface traffic series.
    pub network_io: super::History<Vec<NetworkSnapshot>>,
    /// Number of visible processes.
    pub process_count: super::History<usize>,
}

impl ResourceHistory {
    /// Append one snapshot and trim every series to the requested viewport size.
    pub fn push(&mut self, snapshot: &ResourceSnapshot, capacity: usize) {
        self.cpu_percent.push(snapshot.cpu_percent, capacity);
        self.cpu.push(snapshot.cpu.clone(), capacity);
        self.memory_used_bytes
            .push(snapshot.memory_used_bytes, capacity);
        self.memory_available_bytes
            .push(snapshot.memory_available_bytes, capacity);
        self.memory_cached_bytes
            .push(snapshot.memory_cached_bytes, capacity);
        self.memory_free_bytes
            .push(snapshot.memory_free_bytes, capacity);
        self.swap_used_bytes
            .push(snapshot.swap_used_bytes, capacity);
        self.disk_read_bytes_per_second.push(
            snapshot
                .disks
                .iter()
                .map(|disk| disk.read_bytes_per_second)
                .sum(),
            capacity,
        );
        self.disk_write_bytes_per_second.push(
            snapshot
                .disks
                .iter()
                .map(|disk| disk.write_bytes_per_second)
                .sum(),
            capacity,
        );
        self.disk_io.push(snapshot.disks.clone(), capacity);
        self.network_io.push(snapshot.networks.clone(), capacity);
        self.process_count.push(snapshot.processes.len(), capacity);
    }
}
