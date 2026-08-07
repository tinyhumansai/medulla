//! The Sessions rail's row taxonomy: `Host → Session`.
//!
//! One shape for the whole tree. A row is a host, one session running on it, a
//! workflow run that session started, or one of the two controls — the action
//! that opens a session, and the paging control for a lane whose sessions the
//! fold hid.
//!
//! Two levels have been removed since. `AgentRow::Sub` rendered a *task* and
//! `RailRow::Harness` a *session*, in two groups separated by a
//! `── your harnesses ──` divider; a task **is** an agent session — the two
//! differ only in [`SessionOrigin`](crate::worker::pty::SessionOrigin) — so both
//! collapsed into [`RailRow::Session`]. Then the *agent* tier went the same way:
//! agents are no longer declared from the TUI, and a row for a `harness ×
//! workspace` identity that the operator can neither create nor edit is a level
//! of nesting charged against every session beneath it. Agents still exist —
//! [`AgentRailRow`] is still resolved, and it is still what gives a session its
//! lane — they are simply not rendered.

use medulla::ui::hosts::HostAgentRow;

use crate::ui::agents::TaskState;
use crate::worker::pty::{SessionOrigin, SessionRow};

/// A stable identity for a selectable Sessions-rail row.
///
/// The rail is rebuilt from live state on every frame. Storing an offset would
/// select a different row whenever a row is inserted above it, so the app
/// remembers one of these identities and resolves its current offset instead.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RailAnchor {
    /// A local session, keyed by PTY id.
    Session(String),
    /// A dispatched task without a local PTY row, keyed by its lane and task.
    Task {
        /// Stable key of the lane that owns this task.
        lane: String,
        /// Backend task id within [`Self::Task::lane`].
        task_id: String,
    },
    /// The action that opens a session. There is one, so it carries no key.
    NewSession,
    /// A workflow run, keyed by its run id.
    WorkflowRun(String),
    /// The paging control for a lane, keyed by that lane's stable key.
    Overflow(String),
}

/// One host in the tree.
///
/// Emitted **only when there is a second host to tell apart** (progressive
/// disclosure): with just the local machine — the common case — agents sit at the
/// top level and no host row wraps them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HostRailRow {
    /// The host id agents are stamped with; the local bus address for this
    /// machine.
    pub host_id: String,
    /// What the row says.
    pub label: String,
    /// Whether this is a machine the operator can act on from here — the local
    /// device and any other host declared in this config.
    pub local: bool,
}

/// One agent — `harness × workspace` on a host — resolved but never rendered.
///
/// The rail groups sessions by agent and takes each session's lane from the
/// agent's, so the tier is still assembled; it simply has no row of its own
/// since agents stopped being declared from the TUI. Its label, harness and
/// workspace accessors went with that row.
///
/// Sourced from the shared `Host → Agent` projection
/// ([`medulla::ui::hosts::host_rows`]), which is the same tree the Hosts tab
/// renders: an agent exists because it is declared (or because the roster
/// advertises it), never because traffic happened to fold a lane for it. A lane
/// the fold produced for an agent that projection does not know (a backend-side
/// roster agent, a peer session) still gets a row, so the restructure never
/// hides something that used to be visible.
#[derive(Debug, Clone)]
pub struct AgentRailRow {
    /// The `agentId` a dispatch targets — the key sessions are grouped under.
    pub agent_id: String,
    /// The host this agent runs on. Empty when nothing places it.
    pub host_id: String,
    /// The projection's row for this agent, when the tree knows it. `None` for a
    /// lane-only agent, which is all the fold could tell us about.
    pub agent: Option<HostAgentRow>,
    /// The lane the event fold produced for it, when it has traffic. `None` for
    /// a declared agent that has not run anything.
    pub lane_index: Option<usize>,
}

/// One session of an agent — **one row type, whatever started it**.
///
/// An orchestrator dispatch arrives as a [`TaskState`] folded from the event
/// stream; an operator-started session arrives as a live [`SessionRow`] from the
/// local PTY manager. They are the same thing seen through the two surfaces that
/// can see it, so one row carries either (or, for a dispatch this machine is
/// serving, both).
#[derive(Debug, Clone)]
pub struct SessionRailRow {
    /// The agent this session belongs to. `None` when nothing declares the
    /// directory it runs in — the session is still listed rather than hidden.
    pub agent_id: Option<String>,
    /// The lane the owning agent folded to, for the transcript behind the row.
    pub lane_index: Option<usize>,
    /// The dispatched task, when the orchestrator started this session.
    pub task: Option<TaskState>,
    /// The live PTY session, when this device is the one running it.
    pub local: Option<SessionRow>,
    /// Whether this is the last session listed under its agent, for the tree
    /// glyph.
    pub last: bool,
}

impl SessionRailRow {
    /// The local PTY session id this row names, when it names one.
    pub fn session_id(&self) -> Option<&str> {
        self.local.as_ref().map(|row| row.id.as_str())
    }

    /// Who started this session.
    ///
    /// A row backed by a task is an orchestrator dispatch by construction; a
    /// local-only row reports what the PTY manager recorded at launch.
    pub fn origin(&self) -> SessionOrigin {
        match (&self.task, &self.local) {
            (Some(_), _) => SessionOrigin::Orchestrator,
            (None, Some(row)) => row.origin,
            (None, None) => SessionOrigin::Orchestrator,
        }
    }

    /// The name the operator gave this session, when they gave one.
    pub fn name(&self) -> Option<&str> {
        self.local
            .as_ref()
            .and_then(|row| row.name.as_deref())
            .map(str::trim)
            .filter(|name| !name.is_empty())
    }
}

/// One workflow run a session started over MCP, listed under that session.
///
/// The run itself executes in the MCP subprocess the harness talks to (see
/// [`medulla::control_socket::runs`]), so this row is the only place an
/// operator can see it while it happens — the Workflows page reads the run
/// store, and a record does not land there until the run ends.
#[derive(Debug, Clone)]
pub struct WorkflowRunRailRow {
    /// The PTY session that started it, so the row can be drawn under it and
    /// selection can stay with the harness it belongs to.
    pub session_id: String,
    /// What the reporting process last said about the run.
    pub run: medulla::control_socket::HarnessRun,
    /// Whether this is the last row of its session's group, for the tree glyph.
    pub last: bool,
}

/// One row of the Sessions rail.
#[derive(Debug, Clone)]
pub enum RailRow {
    /// A host header, emitted only once a remote host exists.
    Host(HostRailRow),
    /// One session running on the host above it.
    ///
    /// Boxed because a session row carries both a whole [`TaskState`] and a
    /// whole [`SessionRow`], which together are several times the size of every
    /// other variant — and a rail is a `Vec<RailRow>` rebuilt each frame, so the
    /// widest variant is what every row costs.
    Session(Box<SessionRailRow>),
    /// The action row that opens a session, once per hosting machine.
    ///
    /// One row at the top of the rail, not one per agent. The picker it opens
    /// asks for the harness type and the directory itself, so there is nothing
    /// left for the row's *position* to contribute — and a per-agent copy was
    /// only ever a way of pre-answering those two questions.
    NewSession,
    /// A workflow run one of the sessions above started over MCP.
    ///
    /// Directly beneath the session that triggered it, because that is the
    /// context that makes it legible: the same workflow run means something
    /// different started from a review session than from a release one.
    WorkflowRun(WorkflowRunRailRow),
    /// The `+N more` paging control for a lane whose sessions the fold hid.
    ///
    /// Its own variant rather than a fold row carried through: paging is the
    /// only thing the rail still takes from [`AgentRow`], and keeping the whole
    /// enum for it meant every consumer matched arms — lane headers, dividers,
    /// task sublanes — that the flattened rail can never produce.
    Overflow {
        /// The lane whose page this control opens and closes.
        lane_index: usize,
        /// How many of its sessions are hidden right now.
        hidden: usize,
    },
}

impl RailRow {
    /// Whether the cursor may land on this row.
    pub fn selectable(&self) -> bool {
        match self {
            RailRow::Host(_) => false,
            RailRow::Session(_) => true,
            RailRow::NewSession => true,
            // Selectable so `Enter` can open the workflow it belongs to; the
            // rail is where the operator finds out a run exists, and a row they
            // cannot act on would make them go looking for it by name.
            RailRow::WorkflowRun(_) => true,
            // Selectable so `Enter` pages the lane open, and folds it back once
            // it is fully revealed.
            RailRow::Overflow { .. } => true,
        }
    }

    /// The PTY session this row names, when it names one directly.
    ///
    /// A workflow run row answers `None`, though it knows the session it hangs
    /// under. Every caller asks this to mean "the session this row *is*": the
    /// pane draws that session's terminal, a click attaches the keyboard to it,
    /// and `select_session_row` puts the cursor on it. A run row answering with
    /// its *parent* made all three act on the harness instead of the run — which
    /// is why arrowing onto a run showed the session's terminal rather than the
    /// run, and clicking one attached to a harness the operator had not selected.
    /// The parent is still reachable through
    /// [`workflow_run`](Self::workflow_run) for anything that genuinely wants it.
    pub fn session_id(&self) -> Option<&str> {
        match self {
            RailRow::Session(row) => row.session_id(),
            _ => None,
        }
    }

    /// The workflow run this row is about, when it is about one.
    pub fn workflow_run(&self) -> Option<&WorkflowRunRailRow> {
        match self {
            RailRow::WorkflowRun(row) => Some(row),
            _ => None,
        }
    }

    /// The task this row renders, when it renders one.
    pub fn task(&self) -> Option<&TaskState> {
        match self {
            RailRow::Session(row) => row.task.as_ref(),
            _ => None,
        }
    }

    /// The lane index behind this row, for the transcript pane.
    pub fn lane_index(&self) -> Option<usize> {
        match self {
            RailRow::Session(row) => row.lane_index,
            RailRow::Overflow { lane_index, .. } => Some(*lane_index),
            RailRow::Host(_) | RailRow::NewSession | RailRow::WorkflowRun(_) => None,
        }
    }

    /// Whether this row is the action that opens a session.
    pub fn is_new_session(&self) -> bool {
        matches!(self, RailRow::NewSession)
    }
}
