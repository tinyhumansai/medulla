//! Data types for the `capabilities` module.
#[allow(unused_imports)]
use super::*;
/// Inputs for one capability probe.
pub struct ProbeOptions {
    /// Provider whose capabilities are being probed.
    pub provider: HarnessProvider,
    /// Harness execution callback used for the live probe.
    pub run_task: RunTaskFn,
    /// Primary workspace advertised by the host.
    pub workspace: String,
    /// Additional directories the host makes available.
    pub accessible_dirs: Vec<String>,
    /// Environment inherited by the probe process.
    pub env: HashMap<String, String>,
    /// Complete set of providers detected on the host.
    pub providers: Vec<HarnessProvider>,
    /// Optional probe timeout in milliseconds.
    pub timeout_ms: Option<u64>,
    /// Optional model hint passed to the harness.
    pub model: Option<String>,
    /// Optional agent profile passed to the harness.
    pub agent: Option<String>,
    /// Whether the probe bypasses interactive permission prompts.
    pub skip_permissions: bool,
    /// Cancellation handle for the probe process.
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
    /// Whether commits made by harnesses this probe launches are attributed to
    /// Medulla — the resolved `attribution.commit` config value (on by default;
    /// see [`crate::config::AttributionConfig`]).
    pub attribution: bool,
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
    /// Repository or project name inferred from Git.
    pub project: Option<String>,
    /// Checked-out branch inferred from Git.
    pub branch: Option<String>,
}
