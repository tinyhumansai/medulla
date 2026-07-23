//! Data exchanged by the interactive event loop and its background tasks.

use std::sync::Arc;

use medulla::runtime::ContextItem;

/// Messages sent from spawned async tasks back to the event loop.
pub(super) enum AppMsg {
    /// A status-line update.
    Status(String),
    /// Fresh context-inspection rows.
    Contexts(Vec<ContextItem>),
    /// Best-effort local Git summaries for configured workspace roots.
    WorkspacesLoaded(Vec<medulla::workspace::WorkspaceReport>),
    /// A selected local path's patch (or typed error).
    WorkspaceDiffLoaded {
        /// Local repository root.
        workspace: std::path::PathBuf,
        /// Repository-relative changed path.
        path: std::path::PathBuf,
        /// Patch or displayable failure.
        result: Result<String, String>,
    },
    /// Chats to display in the resume picker.
    OpenResume(Vec<medulla::ui::chat_store::MainChatSummary>),
    /// Confirmation that a chat was resumed.
    Resumed(String),
    /// Memory overview data loaded off the UI thread.
    MemoryLoaded {
        /// Current memory status, when a service is attached.
        status: Option<medulla::memory::MemoryStatus>,
        /// Current persona directives.
        directives: Vec<String>,
    },
    /// Account usage returned by the runtime.
    UsageLoaded(Option<serde_json::Value>),
    /// Ranked memory-search results and their query.
    MemoryResults {
        /// Ranked hits.
        hits: Vec<medulla::memory::MemoryHit>,
        /// The submitted query.
        query: String,
    },
    /// A newer release was detected by the background update checker.
    UpdateAvailable(String),
    /// A page of the feedback board. `None` = this runtime has no board.
    FeedbackLoaded(Option<medulla::client::FeedbackPage>),
    /// Comments for one board item.
    FeedbackComments {
        /// The item the comments belong to.
        id: String,
        /// The item's comments, oldest first.
        comments: Vec<medulla::client::FeedbackComment>,
    },
    /// A board item the server re-tallied after a vote.
    FeedbackItemUpdated(medulla::client::FeedbackItem),
    /// A feedback action finished; reload the board and report `status`.
    FeedbackChanged(String),
    /// A memory ingest finished; clear the in-flight flag and report the outcome.
    MemoryIngestDone(String),
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
    /// The Medulla home, used to locate the credential store.
    pub medulla_home: std::path::PathBuf,
    /// The persona-memory service backing the Memory tab.
    pub memory_service: Option<Arc<medulla::memory::MemoryService>>,
    /// Live events from a history share the welcome flow left running.
    pub sharing:
        Option<tokio::sync::mpsc::UnboundedReceiver<medulla_tui::ui::welcome::WelcomeEvent>>,
    /// Where to record onboarding once a backgrounded share settles.
    pub onboarding_path: std::path::PathBuf,
}
