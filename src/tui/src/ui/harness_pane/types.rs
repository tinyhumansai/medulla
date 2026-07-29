//! Data types for the `harness_pane` module: the handle onto this device's
//! live harnesses, and where the operator's keyboard is pointed.

#[allow(unused_imports)]
use super::*;

/// The local harness sessions the Agents tab reads and types into.
///
/// Two halves that are only useful together: [`sessions`](Self::sessions) holds
/// the screens and the write side, and [`runtime`](Self::runtime) answers which
/// session is serving a given task. Selecting a task in the Agents tab resolves
/// through the runtime and then renders through the manager.
///
/// Cheap to clone — both fields are `Arc`-backed — and a clone does *not* keep
/// the host alive. When the host goes away the sessions simply stop being
/// resolvable, which is the truth the pane should be showing.
#[derive(Clone)]
pub struct LocalHarnesses {
    /// Every PTY-backed harness running on this device.
    pub sessions: PtyManager,
    /// Each host's task state machine, for
    /// [`session_for_task`](medulla::daemon::DaemonRuntime::session_for_task).
    ///
    /// One per host on this device. A task belongs to exactly one of them, so
    /// resolving means asking each until one claims it — which is cheap, since
    /// a machine hosts a handful of directories, not a fleet.
    ///
    /// Shared rather than owned because a host can be added while the app runs.
    /// This value is cloned into the session, so a plain `Vec` would leave the
    /// pane reading a snapshot taken at startup: the new host would run, and its
    /// screen would be unwatchable for the rest of the session.
    pub runtimes: std::sync::Arc<std::sync::Mutex<Vec<medulla::daemon::DaemonRuntime>>>,
    /// The address local work is dispatched *from* — the `from` half of the
    /// `(sender, task id)` key `session_for_task` is keyed on.
    ///
    /// Locally dispatched work carries the hub's own bus address, not the
    /// operator's identity, because the hub is what put the frame on the bus.
    pub hub_address: String,
    /// The environment an operator-started harness is spawned with.
    ///
    /// The same map the host's executor uses, so a harness the operator starts
    /// by hand sees exactly what one started for a task would. Anything else
    /// would make "it works when I run it myself" a real and confusing
    /// difference rather than a figure of speech.
    pub env: std::collections::HashMap<String, String>,
    /// The host's workspace, used as the default directory for a new harness.
    pub workspace: String,
    /// The coding-agent CLIs this device actually has, in the order the picker
    /// should offer them.
    pub providers: Vec<medulla::tinyplace::HarnessProvider>,
    /// The configured `[router]`, injected into an operator-started harness the
    /// same way the executor injects it into a task's.
    pub router: Option<medulla::config::RouterConfig>,
}

/// Where the operator's keystrokes are going.
///
/// The orchestrator TUI is a chrome *around* a real terminal, so at any moment
/// exactly one of the two owns the keyboard. Leaving that implicit is what makes
/// embedded terminals infuriating: the operator types `q` meaning "quit the
/// harness" and quits the wrapper instead, or presses Escape expecting to get
/// out and cancels the harness's turn. So it is explicit, modal, and shown.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum HarnessFocus {
    /// The TUI owns the keyboard. Harness screens still paint live; they just
    /// do not receive input.
    #[default]
    Chrome,
    /// The named session owns the keyboard. Every key is encoded and written to
    /// its PTY except the detach chord, which is the one key the chrome keeps
    /// for itself — without a reserved key there is no way back out.
    Attached(String),
}

impl HarnessFocus {
    /// The session currently receiving keystrokes, if any.
    pub fn attached_to(&self) -> Option<&str> {
        match self {
            HarnessFocus::Chrome => None,
            HarnessFocus::Attached(id) => Some(id.as_str()),
        }
    }

    /// Whether `id` is the session the keyboard is pointed at.
    pub fn is_attached_to(&self, id: &str) -> bool {
        self.attached_to() == Some(id)
    }
}
