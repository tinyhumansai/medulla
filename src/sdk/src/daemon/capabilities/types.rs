//! Data types for the `capabilities` module.
#[allow(unused_imports)]
use super::*;
/// Inputs for one capability probe.
pub struct ProbeOptions {
    pub provider: HarnessProvider,
    pub run_task: RunTaskFn,
    pub workspace: String,
    pub accessible_dirs: Vec<String>,
    pub env: HashMap<String, String>,
    pub providers: Vec<HarnessProvider>,
    pub timeout_ms: Option<u64>,
    pub model: Option<String>,
    pub agent: Option<String>,
    pub skip_permissions: bool,
    pub abort: Abort,
}
pub(super) struct ReportedCapabilities {
    pub(super) accessible_dirs: Vec<String>,
    pub(super) tools: Vec<String>,
    pub(super) mcp_servers: Vec<String>,
    pub(super) summary: Option<String>,
}
/// Git project + branch, best-effort.
#[derive(Debug, Clone, Default)]
pub struct GitFacts {
    pub project: Option<String>,
    pub branch: Option<String>,
}
