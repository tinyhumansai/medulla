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
    pub trust_workspace: bool,
    pub skip_permissions: bool,
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
}
