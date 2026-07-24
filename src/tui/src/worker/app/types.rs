//! Data model for the worker-daemon TUI: its tabs and the [`WorkerApp`]
//! state the render and key layers share.

use std::sync::Arc;

use medulla::contacts::ContactDesk;
use medulla::tinyplace::HarnessProvider;

use super::super::pty::PtyManager;

/// The worker TUI's tabs, in order. Number keys 1-3 jump to them.
///
/// Deliberately three: what the machine is *doing*, who it will *talk to*, and
/// who is *asking*. Anything else belongs in the orchestrator TUI.
pub const TABS: [&str; 3] = ["Sessions", "Contacts", "Requests"];

/// Index of the Sessions tab.
pub const TAB_SESSIONS: usize = 0;
/// Index of the Contacts tab.
pub const TAB_CONTACTS: usize = 1;
/// Index of the Requests tab.
pub const TAB_REQUESTS: usize = 2;

/// How this worker runs the tasks peers send it.
///
/// The distinction is not cosmetic: it decides which executor the daemon runtime
/// is built with, and therefore what there is to look at. Headless runs one
/// process per task and narrates itself in the log; interactive runs a real
/// session you can watch and take over.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExecutionMode {
    /// One-shot per task. Nothing paints; the daemon log is the view.
    Headless,
    /// A live session per conversation, embedded and watchable.
    Interactive,
}

impl ExecutionMode {
    /// The display string.
    pub fn as_str(self) -> &'static str {
        match self {
            ExecutionMode::Headless => "headless",
            ExecutionMode::Interactive => "interactive",
        }
    }

    /// A one-line explanation, shown beside the choice.
    pub fn blurb(self) -> &'static str {
        match self {
            ExecutionMode::Headless => "one-shot per task · logs only",
            ExecutionMode::Interactive => "live sessions you can watch",
        }
    }
}

/// Every mode, in the order the setup step offers them.
pub const EXECUTION_MODES: [ExecutionMode; 2] =
    [ExecutionMode::Headless, ExecutionMode::Interactive];

/// Which question the launch setup step is on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SetupStep {
    /// How tasks are run.
    Mode,
    /// Which coding agent runs them.
    Harness,
}

/// Which screen the worker TUI is showing.
///
/// The daemon has to know which harness powers it before it can accept work:
/// a peer's task frame names a provider only sometimes, and the fallback should
/// be the operator's deliberate choice rather than whichever binary happened to
/// sort first on PATH.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Screen {
    /// The launch step: pick how this worker runs, and on what.
    Setup,
    /// The running worker: sessions, contacts, requests.
    Main,
}

/// A pending confirmation the operator must answer before something destructive
/// happens.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Confirm {
    /// Kill the session with this id.
    CloseSession(String),
    /// Block the peer with this cryptoId.
    BlockPeer(String),
}

impl Confirm {
    /// The question to show.
    pub fn prompt(&self) -> String {
        match self {
            Confirm::CloseSession(id) => {
                format!("Kill session {id}? Its harness loses unsaved work.  y/n")
            }
            Confirm::BlockPeer(peer) => {
                format!("Block {peer}? It cannot request contact again.  y/n")
            }
        }
    }
}

/// A follow-up action the event loop runs off the render thread.
#[derive(Debug, Clone, PartialEq)]
pub enum WorkerCmd {
    /// Quit the TUI (and, being one process, the daemon with it).
    Quit,
    /// Settle an incoming contact request.
    ContactOp {
        /// The requesting peer's cryptoId.
        agent_id: String,
        /// Accept, decline, or block.
        decision: medulla::contacts::ContactDecision,
    },
    /// Poll the relay now rather than waiting out the background interval.
    Refresh,
    /// Setup is answered: build the daemon runtime and start serving peers.
    ///
    /// Emitted once, when the launch step completes. Nothing is listening to the
    /// network until then, which is deliberate — a worker should not accept work
    /// before the operator has said how it should run it.
    Start {
        /// How tasks will be run.
        mode: ExecutionMode,
        /// Which harness will run them.
        provider: HarnessProvider,
    },
}

/// The worker-daemon TUI's state.
pub struct WorkerApp {
    /// Setup step or the running worker.
    pub(super) screen: Screen,
    /// Which setup question is showing.
    pub(super) setup_step: SetupStep,
    /// Cursor on the setup step.
    pub(super) setup_index: usize,
    /// How this worker runs tasks, once chosen.
    pub(super) mode: Option<ExecutionMode>,
    /// The daemon's own log lines — the view in headless mode.
    pub(super) logs: crate::log::LogBuffer,
    /// Scrollback offset in the log pane, in lines from the bottom.
    pub(super) log_scroll: usize,
    /// The harness this worker runs on, once chosen — the provider a peer's task
    /// frame gets when it names none.
    pub(super) harness: Option<HarnessProvider>,
    /// Live harness sessions on this machine.
    pub(super) sessions: PtyManager,
    /// The incoming contact-request queue, when a tiny.place identity is
    /// configured. `None` renders an explainer rather than an empty list.
    pub(super) contacts: Option<ContactDesk>,
    /// This daemon's own tiny.place address, shown so it can be handed to peers.
    pub(super) agent_id: Option<String>,
    /// Which harnesses were found on PATH.
    pub(super) providers: Vec<HarnessProvider>,
    /// The active tab.
    pub(super) tab: usize,
    /// Cursor in the session list.
    pub(super) session_index: usize,
    /// Cursor in the contacts list.
    pub(super) contact_index: usize,
    /// Cursor in the requests list.
    pub(super) request_index: usize,
    /// A pending destructive confirmation.
    pub(super) confirm: Option<Confirm>,
    /// The status line.
    pub(super) status: String,
    /// Whether the loop should exit.
    pub should_quit: bool,
    /// Whether terminal mouse reporting is enabled.
    pub(super) mouse_capture: bool,
    /// Row and horizontal ranges occupied by the rendered tab labels.
    pub(super) hit_tabs: (u16, Vec<(u16, u16)>),
    /// Rendered list rows and the model index represented by their first row.
    pub(super) hit_rows: Option<(ratatui::layout::Rect, usize)>,
    /// Rendered setup choices.
    pub(super) hit_setup: Option<ratatui::layout::Rect>,
    /// The pane the watched terminal was last drawn into, so its PTY is resized
    /// to what the operator is actually looking at.
    pub(super) terminal_area: ratatui::layout::Rect,
    /// A clock, injectable for tests.
    pub(super) now: Arc<dyn Fn() -> i64 + Send + Sync>,
    /// Test-only clipboard capture: when set, copying records here and skips the
    /// platform writers, so no test shells out to `pbcopy` or writes OSC to a
    /// terminal that is not there.
    pub(super) copy_capture: Option<Arc<std::sync::Mutex<Vec<String>>>>,
}

impl WorkerApp {
    /// How this worker runs tasks, once chosen. Test/inspection seam.
    pub fn mode(&self) -> Option<ExecutionMode> {
        self.mode
    }

    /// Which setup question is showing. Test/inspection seam.
    pub fn setup_step(&self) -> SetupStep {
        self.setup_step
    }

    /// The captured daemon log.
    pub fn logs(&self) -> &crate::log::LogBuffer {
        &self.logs
    }

    /// Which screen is showing. Test/inspection seam.
    pub fn screen(&self) -> Screen {
        self.screen
    }

    /// The harness powering this worker, once chosen.
    pub fn harness(&self) -> Option<HarnessProvider> {
        self.harness
    }

    /// The active tab's name.
    pub fn tab(&self) -> &'static str {
        TABS[self.tab.min(TABS.len() - 1)]
    }

    /// The status line text. Test/inspection seam.
    pub fn status(&self) -> &str {
        &self.status
    }

    /// Whether the event loop should ask the terminal to report mouse input.
    pub fn mouse_capture(&self) -> bool {
        self.mouse_capture
    }

    /// The pending confirmation, if any. Test/inspection seam.
    pub fn confirm(&self) -> Option<&Confirm> {
        self.confirm.as_ref()
    }

    /// The live session manager, for the event loop.
    pub fn sessions(&self) -> &PtyManager {
        &self.sessions
    }

    /// Set the status line.
    pub fn set_status(&mut self, status: impl Into<String>) {
        self.status = status.into();
    }
}
