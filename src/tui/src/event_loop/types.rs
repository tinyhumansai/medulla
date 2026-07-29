//! Data exchanged by the interactive event loop and its background tasks.

use std::sync::Arc;

use medulla::runtime::ContextItem;

/// Messages sent from spawned async tasks back to the event loop.
pub(super) enum AppMsg {
    /// A status-line update.
    Status(String),
    /// Fresh context-inspection rows.
    Contexts(Vec<ContextItem>),
    /// Chats to display in the resume picker.
    OpenResume(Vec<medulla::ui::chat_store::MainChatSummary>),
    /// Confirmation that a chat was resumed.
    Resumed(String),
    /// The session was cleared; quit back to the login screen.
    LoggedOut,
    /// Account usage returned by the runtime.
    UsageLoaded(Option<serde_json::Value>),
    /// A newer release was detected by the background update checker.
    UpdateAvailable(String),
    /// Current local task document.
    TasksLoaded(medulla::tasks::TaskDocument),
    /// A progress line from a running copilot turn.
    ///
    /// Addressed by workflow rather than applied to whatever is selected: the
    /// operator may have moved the rail on while the turn runs, and the line
    /// belongs to the thread that asked for it.
    #[cfg(feature = "workflows")]
    CopilotStatus {
        /// The workflow whose turn reported it.
        workflow: String,
        /// The progress line.
        line: String,
    },
    /// A copilot turn finished.
    #[cfg(feature = "workflows")]
    CopilotDone {
        /// The workflow the turn was scoped to.
        workflow: String,
        /// The agent's reply.
        reply: String,
        /// What the turn changed in the stored graph.
        changes: Vec<String>,
        /// The workflow the turn created, for a create turn that made one.
        created: Option<String>,
        /// Whether the workflow the turn was scoped to no longer exists, so its
        /// conversation can be closed down with it.
        removed: bool,
    },
    /// A copilot turn failed.
    #[cfg(feature = "workflows")]
    CopilotFailed {
        /// The workflow the turn was scoped to.
        workflow: String,
        /// Why it failed.
        error: String,
    },
    /// The workflow store was written to by something other than a copilot turn,
    /// so the catalogue on screen is stale.
    ///
    /// Carries no payload: what changed is whatever the store now holds, and a
    /// re-read is both cheaper and more honest than describing the edit twice.
    #[cfg(feature = "workflows")]
    WorkflowsChanged,
}

/// Why the event loop stopped.
///
/// A logout is not an exit: it tears the authenticated session down but expects
/// the caller to return to the login screen rather than to the shell.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SessionExit {
    /// The user quit; the process should exit.
    Quit,
    /// The user logged out; re-authenticate and start a fresh session.
    Relogin,
}

/// Everything a session needs besides the terminal and the runtime.
///
/// Bundled rather than passed positionally: these are all "wire this into the
/// app" values, and a session is started afresh on every relogin, so the call
/// site reads better as one named record than as eight arguments.
pub(crate) struct SessionWiring {
    /// The loaded configuration for this session.
    pub loaded: medulla::config::LoadedConfig,
    /// A note to show on the status line at startup, if any.
    pub startup_status: Option<String>,
    /// The tiny.place presence observation, when that service is running.
    pub tinyplace_obs:
        Option<Arc<std::sync::Mutex<medulla::tinyplace::service::TinyplaceObservation>>>,
    /// Where appearance/config edits are persisted.
    pub config_path: std::path::PathBuf,
    /// The Medulla home: where local task/appearance state is kept.
    pub medulla_home: std::path::PathBuf,
    /// The account the embedded core is signed in as, when it is.
    ///
    /// Resolved once at startup rather than polled: the session cannot change
    /// under a running app — logging out quits it.
    pub account: Option<medulla::core_host::auth::AuthState>,
    /// Live events from a history share the welcome flow left running.
    pub sharing:
        Option<tokio::sync::mpsc::UnboundedReceiver<medulla_tui::ui::welcome::WelcomeEvent>>,
    /// Where to record onboarding once a backgrounded share settles.
    pub onboarding_path: std::path::PathBuf,
    /// A read-only view of the host running on this device, when one is. `None`
    /// means this machine orchestrates but does not run the work itself.
    pub host: Option<medulla::daemon::embedded::HostObservation>,
    /// The live harness sessions this device is running, and the state machine
    /// that says which task each one serves.
    ///
    /// `None` when this machine does not host: there are no local harnesses to
    /// show, and the Agents tab falls back to a remote worker's streamed screen
    /// or to the transcript. Shared with the host's executor — the sessions it
    /// opens are the ones rendered here.
    pub harnesses: Option<medulla_tui::ui::harness_pane::LocalHarnesses>,
}
