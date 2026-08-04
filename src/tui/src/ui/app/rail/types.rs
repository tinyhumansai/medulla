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

use medulla::runtime::AgentDeclaration;

use crate::ui::agents::{AgentRow, TaskState};
use crate::worker::pty::{SessionOrigin, SessionRow};

/// One host in the tree.
///
/// Emitted **only when a remote host exists** (progressive disclosure): with just
/// the local machine — the common case — agents sit at the top level and no host
/// row wraps them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HostRailRow {
    /// The host id agents are stamped with; the local bus address for this
    /// machine.
    pub host_id: String,
    /// What the row says.
    pub label: String,
    /// Whether this is the machine the TUI is running on.
    pub local: bool,
}

/// One agent in the tree — `harness × workspace` on a host.
///
/// Sourced from the **declaration**, not from the event fold: an agent that has
/// never been dispatched to still has a row, which is the whole point of
/// declaring one. A lane the fold produced for an agent nothing declares (a
/// tiny.place peer, an agent advertised by another machine) still gets a row, so
/// the restructure never hides something that used to be visible.
#[derive(Debug, Clone)]
pub struct AgentRailRow {
    /// The `agentId` a dispatch targets — the key sessions are grouped under.
    pub agent_id: String,
    /// The host this agent runs on. Empty when nothing places it.
    pub host_id: String,
    /// The declaration this row came from, when the agent is declared here.
    pub declaration: Option<AgentDeclaration>,
    /// The lane the event fold produced for it, when it has traffic. `None` for
    /// a declared agent that has not run anything.
    pub lane_index: Option<usize>,
}

impl AgentRailRow {
    /// The label an undeclared, laneless row would show — the id itself.
    ///
    /// A declared agent prefers its operator-chosen name, then the folder its
    /// workspace sits in, because that is how a person refers to it.
    pub fn label(&self) -> String {
        let Some(declaration) = &self.declaration else {
            return self.agent_id.clone();
        };
        if let Some(name) = declaration.name.as_deref().map(str::trim) {
            if !name.is_empty() {
                return name.to_string();
            }
        }
        self.agent_id.clone()
    }

    /// The harness type this agent runs, when a declaration says.
    pub fn harness(&self) -> Option<&str> {
        self.declaration
            .as_ref()
            .map(|declaration| declaration.harness.as_str())
    }

    /// The directory this agent's sessions work in, when a declaration says.
    pub fn workspace(&self) -> Option<&str> {
        self.declaration
            .as_ref()
            .and_then(|declaration| declaration.workspace.path())
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
    /// The action row that declares a new agent on this machine.
    ///
    /// Sits directly above the tree it produces, because a machine with no
    /// agents declared has nothing else on that half of the rail to suggest the
    /// flow exists — which is the same as not having it.
    NewAgent,
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
            RailRow::Lane(row) => row.selectable(),
        }
    }

    /// The PTY session this row names, when it names one directly.
    pub fn session_id(&self) -> Option<&str> {
        match self {
            RailRow::Session(row) => row.session_id(),
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
            RailRow::Host(_) | RailRow::NewAgent => None,
        }
    }

    /// The agent this row is about, when it is about one.
    pub fn agent_id(&self) -> Option<&str> {
        match self {
            RailRow::Agent(row) => Some(row.agent_id.as_str()),
            RailRow::Session(row) => row.agent_id.as_deref(),
            _ => None,
        }
    }

    /// Whether this row is the "declare an agent" action.
    pub fn is_new_agent(&self) -> bool {
        matches!(self, RailRow::NewAgent)
    }
}
