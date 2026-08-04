//! Data types for the Hosts surface: a host and the agents under it.

/// Where a host is, and therefore what an operator may do to it from here.
///
/// The v1 capability split (spec §2.4): agents are created and edited on the
/// machine that owns them, so a remote host's agents are read-only here. This is
/// about the *operator's* affordances only — the orchestrator dispatches to a
/// remote agent exactly as it does to a local one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HostKind {
    /// A host this machine runs: `[host]` or one of the `[[hosts]]` entries.
    /// Its agents are declared in this config, so they can be edited here.
    Local,
    /// Another machine, reached by its tiny.place address. Its agents are
    /// declared over there.
    Remote,
}

impl HostKind {
    /// Whether an operator may declare or edit agents on this host from here.
    pub fn editable(self) -> bool {
        matches!(self, HostKind::Local)
    }
}

/// One host in the Hosts tab, with the agents known to be on it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HostRow {
    /// The `hostId`: the bus address a local host binds, or the tiny.place
    /// address a remote one is reached at.
    pub id: String,
    /// What to call it on screen.
    pub label: String,
    /// Local or remote — see [`HostKind`].
    pub kind: HostKind,
    /// The agents under it, in declaration order for a local host and roster
    /// order for a remote one.
    pub agents: Vec<HostAgentRow>,
    /// The roster entry whose capability probe describes this *machine*
    /// (capacity, readiness, budgets). `None` when nothing on this host is in
    /// the roster — a host declared here but not running.
    ///
    /// Capacity is a property of the machine, not of one agent, so the preview
    /// reads it from whichever entry reported it rather than repeating it under
    /// every agent.
    pub detail_worker: Option<String>,
}

impl HostRow {
    /// Whether the operator may declare a new agent on this host from here.
    pub fn accepts_new_agents(&self) -> bool {
        self.kind.editable()
    }
}

/// One agent under a host.
///
/// Two sources feed it, and the difference is what the tab must not blur. A
/// **declared** agent is written down in this machine's `[fleet]`: it is an
/// agent whether or not anything is running, and its roles are editable here. An
/// **undeclared** one is only known because the roster has an entry for it —
/// a migration seed on this machine, or a remote peer whose declarations this
/// hub cannot see until the host link learns to exchange them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HostAgentRow {
    /// The `agentId` a dispatch targets. Also the roster/worker id.
    pub agent_id: String,
    /// What to call it: its declared name, its roster label, else its id.
    pub label: String,
    /// The coding-agent CLI it runs, when known.
    pub harness: Option<String>,
    /// The directory its sessions work in, when known.
    pub workspace: Option<String>,
    /// Agent-template ids it is offered for. Empty means unspecified — a
    /// general agent, not one excluded from every role.
    pub roles: Vec<String>,
    /// Sessions it may run at once, derived from its declared strategy. `None`
    /// for an agent this machine has not declared.
    pub max_sessions: Option<u32>,
    /// Whether this machine's config declares it.
    pub declared: bool,
    /// Whether its roles can be assigned here. True only on a local host: a
    /// remote host's agents are declared on that machine.
    pub editable: bool,
    /// Whether the live roster carries an entry for it — i.e. whether the
    /// orchestrator can dispatch to it right now.
    pub live: bool,
    /// Whether it is the operator's manually-selected default worker.
    pub selected: bool,
}
