//! Data types for the `state` module.
#[allow(unused_imports)]
use super::*;
/// How the worker TUI is wired at startup.
pub struct WorkerWiring {
    /// The session manager (shared with the daemon's inbound path).
    pub sessions: PtyManager,
    /// This daemon's own link node name.
    pub agent_id: Option<String>,
    /// Harnesses detected on PATH.
    pub providers: Vec<HarnessProvider>,
    /// A note to show on the status line at startup.
    pub startup_status: Option<String>,
    /// Where the daemon's log lines are captured.
    pub logs: LogBuffer,
    /// Primary directory where inbound tasks execute.
    pub primary_workspace: String,
    /// Workspace roots approved for capability advertisement.
    pub workspaces: Vec<String>,
    /// Configured orchestrator/master peers.
    pub masters: Vec<Peer>,
    /// Config file this screen owns for persistence.
    pub config_path: std::path::PathBuf,
    /// Worker-local wallet directory; displayed as the credential boundary.
    pub credential_dir: std::path::PathBuf,
    /// Relay endpoint resolved at startup.
    pub endpoint: Option<String>,
    /// Shared visual theme used by the main and daemon TUIs.
    pub theme: Theme,
}
