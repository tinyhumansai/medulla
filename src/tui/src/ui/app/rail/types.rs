//! The Agents rail's row taxonomy: `Host → Agent → Session`.
//!
//! One shape for the whole tree. A row is a host, an agent, one of that agent's
//! sessions, or the action that declares a new agent — and the lane rows the
//! event fold still owns for the surfaces that are *not* agents (the
//! orchestrator's own conversation, the `── functions ──` divider, the function
//! lanes beneath it).
//!
//! The taxonomy this replaced carried the split the redefinition removes: an
//! `AgentRow::Sub` rendered a *task* and a `RailRow::Harness` rendered a
//! *session*, in two groups separated by a `── your harnesses ──` divider. A
//! task **is** an agent session — the two differ only in
//! [`SessionOrigin`](crate::worker::pty::SessionOrigin) — so both collapse into
//! [`RailRow::Session`] under the agent that owns them, and the divider is gone.

use medulla::ui::hosts::HostAgentRow;

use crate::ui::agents::{AgentRow, TaskState};
use crate::worker::pty::{SessionOrigin, SessionRow};

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

/// One agent in the tree — `harness × workspace` on a host.
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

impl AgentRailRow {
    /// What to call this agent: whatever the shared projection resolved (its
    /// declared name, else its roster label), falling back to the id itself.
    pub fn label(&self) -> String {
        self.agent
            .as_ref()
            .map(|agent| agent.label.clone())
            .filter(|label| !label.trim().is_empty())
            .unwrap_or_else(|| self.agent_id.clone())
    }

    /// The harness type this agent runs, when the tree knows one.
    pub fn harness(&self) -> Option<&str> {
        self.agent.as_ref()?.harness.as_deref()
    }

    /// The directory this agent's sessions work in, when the tree knows one.
    pub fn workspace(&self) -> Option<&str> {
        self.agent.as_ref()?.workspace.as_deref()
    }
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

/// One row of the Agents rail.
#[derive(Debug, Clone)]
pub enum RailRow {
    /// A host header, emitted only once a remote host exists.
    Host(HostRailRow),
    /// A declared (or folded) agent.
    Agent(AgentRailRow),
    /// One session of the agent above it.
    ///
    /// Boxed because a session row carries both a whole [`TaskState`] and a
    /// whole [`SessionRow`], which together are several times the size of every
    /// other variant — and a rail is a `Vec<RailRow>` rebuilt each frame, so the
    /// widest variant is what every row costs.
    Session(Box<SessionRailRow>),
    /// The heading over the agent tree.
    ///
    /// Sits under the create action, so the rail reads as "here is the button,
    /// and here is what it has made". Without it the first agent row followed
    /// the button directly and the two read as one block — a list whose first
    /// entry happened to be green.
    ///
    /// Emitted only when there is a tree to head: a heading over nothing
    /// announces a section the operator cannot see.
    AgentsHeader,
    /// The action row that declares a new agent on this machine.
    ///
    /// Sits directly above the tree it produces, because a machine with no
    /// agents declared has nothing else on that half of the rail to suggest the
    /// flow exists — which is the same as not having it.
    NewAgent,
    /// The action row that opens a new session under the agent above it.
    ///
    /// The last row of an agent's group, under its sessions, because that is
    /// where "and one more" belongs: the list you are reading is what the agent
    /// is running, and this adds to it. Emitted only for an agent this machine
    /// declares — a session is started on the host that owns the agent, so
    /// offering the action on a remote one would be a button that refuses.
    ///
    /// `open_new_session` has existed since the tree landed and was reachable
    /// only by `Ctrl-T`, i.e. only by an operator who already knew it was there.
    NewSession {
        /// The agent whose harness and workspace the session inherits.
        agent_id: String,
    },
    /// A workflow run one of the sessions above started over MCP.
    ///
    /// Directly beneath the session that triggered it, because that is the
    /// context that makes it legible: the same workflow run means something
    /// different started from a review session than from a release one.
    WorkflowRun(WorkflowRunRailRow),
    /// A fold row that is not an agent: the orchestrator's own conversation, the
    /// `── functions ──` divider, a function lane, or a `+N more` counter.
    ///
    /// The one variant the topology does not name, because the orchestrator is
    /// not an agent and still needs somewhere to live. Everything in it is
    /// either the conversation or a label.
    Lane(AgentRow),
}

impl RailRow {
    /// Whether the cursor may land on this row.
    pub fn selectable(&self) -> bool {
        match self {
            RailRow::Host(_) => false,
            RailRow::Agent(_) => true,
            RailRow::Session(_) => true,
            RailRow::NewAgent => true,
            // A label, not a row: the cursor skips it like a host header.
            RailRow::AgentsHeader => false,
            RailRow::NewSession { .. } => true,
            // Selectable so `Enter` can open the workflow it belongs to; the
            // rail is where the operator finds out a run exists, and a row they
            // cannot act on would make them go looking for it by name.
            RailRow::WorkflowRun(_) => true,
            RailRow::Lane(row) => row.selectable(),
        }
    }

    /// The PTY session this row names, when it names one directly.
    pub fn session_id(&self) -> Option<&str> {
        match self {
            RailRow::Session(row) => row.session_id(),
            // A run row names the session it hangs under: the harness is what
            // the operator is looking at, and a run is something it did.
            RailRow::WorkflowRun(row) => Some(row.session_id.as_str()),
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
            RailRow::Agent(row) => row.lane_index,
            RailRow::Session(row) => row.lane_index,
            RailRow::Lane(row) => row.lane_index(),
            RailRow::Host(_)
            | RailRow::NewAgent
            | RailRow::AgentsHeader
            | RailRow::NewSession { .. }
            | RailRow::WorkflowRun(_) => None,
        }
    }

    /// The agent this row is about, when it is about one.
    pub fn agent_id(&self) -> Option<&str> {
        match self {
            RailRow::Agent(row) => Some(row.agent_id.as_str()),
            RailRow::Session(row) => row.agent_id.as_deref(),
            RailRow::NewSession { agent_id } => Some(agent_id.as_str()),
            _ => None,
        }
    }

    /// Whether this row is the "declare an agent" action.
    pub fn is_new_agent(&self) -> bool {
        matches!(self, RailRow::NewAgent)
    }

    /// The agent a "start a session" action row would open one under.
    pub fn new_session_agent(&self) -> Option<&str> {
        match self {
            RailRow::NewSession { agent_id } => Some(agent_id.as_str()),
            _ => None,
        }
    }
}
