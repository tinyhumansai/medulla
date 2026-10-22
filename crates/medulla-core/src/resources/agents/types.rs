use std::path::PathBuf;

/// A recognized coding agent or supported shell process.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum AgentKind {
    Claude,
    Codex,
    CodexAppServer,
    ClaudeAcp,
    CodexAcp,
    Opencode,
    OpencodeAcp,
    OpenhumanCore,
    EmbeddedOpenhuman,
    Shell,
    /// A supported executable whose command-line signature could not be read.
    Unknown,
}

/// Ownership source for a discovered process tree.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum AgentOrigin {
    Session { id: String },
    Task { id: String },
    Headless,
    Remote { host: String },
    External,
}

/// Resource counters associated with one process.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct ProcessUsage {
    /// CPU use as a percentage of one logical CPU.
    pub cpu_percent: f32,
    /// Resident memory in bytes.
    pub rss_bytes: u64,
    /// Number of OS threads.
    pub threads: u64,
    /// Cumulative IO counters, when available.
    pub io: Option<IoCounters>,
}

/// Cumulative process IO counters.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct IoCounters {
    /// Bytes read since process start, if reported.
    pub read_bytes: u64,
    /// Bytes written since process start, if reported.
    pub written_bytes: u64,
}

/// One platform-neutral row from a process sampler.
#[derive(Debug, Clone, PartialEq)]
pub struct ProcessRecord {
    pub pid: u32,
    pub ppid: Option<u32>,
    pub exe: Option<PathBuf>,
    /// `None` means argv could not be read (for example, macOS privacy rules).
    pub argv: Option<Vec<String>>,
    pub cwd: Option<PathBuf>,
    /// Process start time in seconds since the Unix epoch.
    pub started_at_secs: u64,
    pub usage: ProcessUsage,
}

/// A process root whose ownership is known to Medulla.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KnownProcessRoot {
    pub pid: u32,
    pub started_at_secs: u64,
    pub origin: AgentOrigin,
}

/// Controls optional agent categories in process discovery.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct AgentDiscoveryOptions {
    /// Include external shells in addition to shells owned by known Medulla roots.
    pub show_external_shells: bool,
}

/// A descendant in an agent process tree.
#[derive(Debug, Clone, PartialEq)]
pub struct ProcessNode {
    pub pid: u32,
    pub exe: Option<PathBuf>,
    pub usage: ProcessUsage,
    pub children: Vec<ProcessNode>,
}

/// A classified agent process and its aggregate resource usage.
#[derive(Debug, Clone, PartialEq)]
pub struct AgentProcess {
    /// Root process identifier. `None` for an in-process agent such as the
    /// embedded OpenHuman runtime.
    pub pid: Option<u32>,
    pub kind: AgentKind,
    pub origin: AgentOrigin,
    pub exe: Option<PathBuf>,
    pub cwd: Option<PathBuf>,
    pub started_at_secs: u64,
    pub runtime_secs: u64,
    pub usage: ProcessUsage,
    pub children: Vec<ProcessNode>,
}
