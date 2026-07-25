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
    /// Operator-declared `[budget]` config. When set, matching providers advertise
    /// `source: configured` budgets instead of estimates. `None` leaves every
    /// harness on a best-effort estimate.
    pub budget: Option<crate::config::BudgetConfig>,
    /// The daemon's custom OpenAI-compatible router. The probe spawns a real
    /// harness inference, so it must route exactly like a delegated task: when a
    /// gateway is configured (and, with `apiKeyEnv`, the only credential the
    /// harness has), spawning with `None` would either send this machine's context
    /// straight to the provider on ambient credentials or fail to authenticate.
    /// `None` leaves routing off.
    pub router: Option<crate::config::RouterConfig>,
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
