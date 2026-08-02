//! Data types for the `worker_loop` module.
#[allow(unused_imports)]
use super::*;
/// Everything the daemon TUI needs after identity/bootstrap resolution.
pub struct WorkerTuiConfig {
    pub env: HashMap<String, String>,
    pub workspace: String,
    pub workspaces: Vec<String>,
    pub masters: Vec<medulla::config::Peer>,
    pub config_path: std::path::PathBuf,
    pub credential_dir: std::path::PathBuf,
    pub contacts: Option<ContactDesk>,
    pub agent_id: Option<String>,
    pub startup_status: Option<String>,
    pub transport: Option<SignalTransport>,
    pub endpoint: Option<String>,
    pub theme: medulla_tui::ui::theme::Theme,
    pub trust_workspace: bool,
    pub skip_permissions: bool,
    /// Custom OpenAI-compatible router loaded from the layered config. Threaded
    /// into every peer task's spawn environment so the worker TUI routes exactly
    /// like the headless daemon. `None` means routing is off.
    pub router: Option<medulla::config::RouterConfig>,
    /// Operator-declared per-provider token budgets from the `[budget]` config,
    /// advertised on the capability probe as `source: configured`. `None` means
    /// estimates only.
    pub budget: Option<medulla::config::BudgetConfig>,
    /// Whether commits made by harnesses this worker launches are attributed
    /// to Medulla — the resolved `attribution.commit` config value (on by
    /// default; see [`medulla::config::AttributionConfig`]).
    pub attribution: bool,
}
/// The select loop.
/// What building the daemon runtime needs, once the launch step is answered.
pub(in super::super) struct StartWiring {
    pub(in super::super) env: HashMap<String, String>,
    pub(in super::super) workspace: String,
    pub(in super::super) workspaces: Vec<String>,
    pub(in super::super) providers: Vec<HarnessProvider>,
    pub(in super::super) sessions: PtyManager,
    pub(in super::super) transport: Option<SignalTransport>,
    pub(in super::super) logs: LogBuffer,
    /// Whether to pre-trust the workspace with claude. `--no-trust-workspace`
    /// clears it for an operator who would rather answer the dialog themselves.
    pub(in super::super) trust_workspace: bool,
    /// Whether peer sessions run with the harness's permission-bypass flag.
    /// `--no-skip-permissions` clears it.
    pub(in super::super) skip_permissions: bool,
    /// Custom OpenAI-compatible router from the loaded config, layered into each
    /// peer task's spawn environment. `None` means routing is off.
    pub(in super::super) router: Option<medulla::config::RouterConfig>,
    /// Operator-declared per-provider token budgets from the loaded config,
    /// advertised on the capability probe. `None` means estimates only.
    pub(in super::super) budget: Option<medulla::config::BudgetConfig>,
    /// Whether commits made by harnesses this worker launches are attributed
    /// to Medulla — the resolved `attribution.commit` config value (on by
    /// default; see [`medulla::config::AttributionConfig`]).
    pub(in super::super) attribution: bool,
}
