//! Operator-declared agents: the writable end of the fleet chain.
//!
//! The rest of this module describes capacity a *manager* reported —
//! `Host → Harness → Workspace → Agent` as the backend sees it. This file is the
//! other direction: what the operator declares on **this machine**, which is
//! then the source of the local roster rather than a decoration beside it.
//!
//! An agent is `harness × workspace`: the CLI that runs the work and the folder
//! it runs in. One machine holds as many as you declare — the
//! one-advertised-worker-per-machine collapse is what having a declaration
//! replaces. Declared, never probed: nothing here is discovered by scanning
//! directories, and the only automatic declarations are the migration seeds
//! ([`seed_declarations`]), which exist so an install that predates declarations
//! keeps the roster it already had.
//!
//! Storage is `[fleet].agentDeclarations` in the operator's config file; the
//! reading, writing and query helpers live in
//! [`crate::config`](crate::config::declare_agent).

use serde::{Deserialize, Serialize};

/// The workspace type declared when the operator did not say.
///
/// The type is free-form on purpose — a deployment may have a vocabulary this
/// client has never heard of, and rejecting it would make the fleet unreadable
/// rather than the value unused. The suggested vocabulary is
/// `checkout` · `worktree` · `scratch` · `container`.
pub const WORKSPACE_TYPE_CHECKOUT: &str = "checkout";

/// Sessions a `worktree`-strategy agent may run at once.
///
/// A placeholder, not a measurement: worktree provisioning is not implemented
/// (see [`WorkspaceStrategy::selectable`]), so nothing derives this number from
/// real per-session worktrees yet. It exists so the strategy → capacity
/// derivation is total, and the provisioning follow-up owns replacing it.
pub const WORKTREE_MAX_SESSIONS: u32 = 4;

/// Where an agent's sessions do their work.
///
/// `path` is absolute as the declaring machine sees it; `kind` is the free-form
/// workspace type (see [`WORKSPACE_TYPE_CHECKOUT`]). The pair travels together
/// because the path alone cannot say whether two agents sharing it are sharing a
/// checkout — which is exactly what bounds their concurrency.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct WorkspaceRef {
    /// Absolute filesystem path, as seen by the host that declared it.
    pub path: String,
    /// Free-form workspace type: `checkout` · `worktree` · `scratch` · … .
    ///
    /// Named `kind` in Rust and `type` on the wire, because `type` is a
    /// keyword; the serialized form is what the spec and the backend use.
    #[serde(rename = "type", default = "default_workspace_type")]
    pub kind: String,
}

/// The workspace type an undeclared `kind` falls back to.
fn default_workspace_type() -> String {
    WORKSPACE_TYPE_CHECKOUT.to_string()
}

impl Default for WorkspaceRef {
    fn default() -> Self {
        Self {
            path: String::new(),
            kind: default_workspace_type(),
        }
    }
}

impl WorkspaceRef {
    /// A checkout-typed workspace at `path` — the common declaration.
    pub fn checkout(path: impl Into<String>) -> Self {
        Self {
            path: path.into(),
            kind: default_workspace_type(),
        }
    }

    /// The path, or `None` when it is blank.
    ///
    /// Blank is not a placement: advertising `workspace: ""` tells the backend
    /// the agent works at the filesystem root, where a bare "unknown" would have
    /// let it fall back to the worker's probed cwd.
    pub fn path(&self) -> Option<&str> {
        let path = self.path.trim();
        (!path.is_empty()).then_some(path)
    }
}

/// How an agent's sessions get a working copy — and therefore how many of them
/// may run at once.
///
/// Concurrency is *derived* from this rather than configured: a number an
/// operator can raise past what the workspace can actually take is a number that
/// corrupts a checkout.
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum WorkspaceStrategy {
    /// Every session shares the agent's one workspace directory, so exactly one
    /// session writes at a time and the next queues. The v1 default and the only
    /// implemented strategy.
    #[default]
    Checkout,
    /// Each session gets a carved per-session git worktree, so sessions run
    /// genuinely in parallel.
    ///
    /// **Not selectable in v1**: the variant, its wire value and its capacity
    /// derivation exist so the model is complete and a config that names it
    /// still parses, but nothing provisions a worktree yet. Offering it in a
    /// picker would declare parallel sessions that all land in one directory.
    /// See [`selectable`](Self::selectable).
    Worktree,
}

impl WorkspaceStrategy {
    /// Sessions this strategy permits at once. `Checkout` ⇒ 1 (serial).
    pub fn max_sessions(self) -> u32 {
        match self {
            Self::Checkout => 1,
            Self::Worktree => WORKTREE_MAX_SESSIONS,
        }
    }

    /// The wire/config spelling.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Checkout => "checkout",
            Self::Worktree => "worktree",
        }
    }

    /// Whether an operator may choose this strategy today.
    ///
    /// The UI offers [`SELECTABLE_STRATEGIES`]; this is the same answer for a
    /// strategy already on disk, so a hand-written `worktree` can be shown as
    /// declared without becoming an option in a picker.
    pub fn selectable(self) -> bool {
        matches!(self, Self::Checkout)
    }
}

/// The strategies a create-agent flow may offer, in display order.
///
/// One list, so the picker and the "is this implemented" check cannot disagree.
pub const SELECTABLE_STRATEGIES: &[WorkspaceStrategy] = &[WorkspaceStrategy::Checkout];

/// One agent an operator has declared on a host.
///
/// This is the roster's source: [`crate::hub::WorkerSpec`] and the advert are
/// projections of it, never the other way round. Identity is `agent_id` — the
/// token a dispatch targets — and it is stable across restarts because it is
/// written down rather than derived from whatever the machine happened to have
/// installed at boot.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct AgentDeclaration {
    /// The `agentId` a dispatch targets. Unique within the fleet.
    pub agent_id: String,
    /// The host this agent runs on — the local bus address for a device-local
    /// host, matching the `[host]`/`[[hosts]]` entry that binds it.
    #[serde(default)]
    pub host_id: String,
    /// The coding-agent CLI that runs the work: `claude` · `codex` · `opencode`
    /// or a custom harness id. This is the only surviving sense of "harness" —
    /// a type on an agent, never an entity of its own.
    pub harness: String,
    /// Where its sessions work.
    #[serde(default)]
    pub workspace: WorkspaceRef,
    /// Operator-chosen display name. `None` leaves naming to the renderer,
    /// which is what keeps a single-agent machine reading as "this device"
    /// instead of a label nobody typed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// Agent-template ids this agent is offered for. Empty means unspecified —
    /// a general agent, not one excluded from everything.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub roles: Vec<String>,
    /// How its sessions get a working copy, and so how many may run at once.
    #[serde(default)]
    pub strategy: WorkspaceStrategy,
}

impl AgentDeclaration {
    /// A checkout-strategy agent: the shape every v1 declaration has.
    pub fn new(
        agent_id: impl Into<String>,
        host_id: impl Into<String>,
        harness: impl Into<String>,
        workspace: impl Into<String>,
    ) -> Self {
        Self {
            agent_id: agent_id.into(),
            host_id: host_id.into(),
            harness: harness.into(),
            workspace: WorkspaceRef::checkout(workspace),
            name: None,
            roles: Vec::new(),
            strategy: WorkspaceStrategy::Checkout,
        }
    }

    /// Sessions this agent may run at once, derived from its strategy.
    pub fn max_sessions(&self) -> u32 {
        self.strategy.max_sessions()
    }

    /// Whether this declaration belongs to `host_id`.
    ///
    /// Trimmed on both sides: a host id is typed into a config file, and a
    /// trailing space must not silently detach every agent from its machine.
    pub fn on_host(&self, host_id: &str) -> bool {
        self.host_id.trim() == host_id.trim()
    }
}

/// Declarations standing in for an install that has none — the migration seed.
///
/// **Seeding rule.** A machine that has never declared an agent must not go from
/// "one worker in the roster" to "nothing in the roster" on upgrade, so on first
/// load with no declarations for a host, one is synthesized per harness the
/// daemon detected, all at that host's workspace, `strategy: checkout`, no roles
/// and no name. Two properties make this a migration rather than a guess:
///
/// - the **default harness keeps the host's own id**, so the agent that already
///   existed is advertised under exactly the id, address and label it had — ids
///   remembered elsewhere keep resolving;
/// - every other detected harness becomes `{host_id}-{harness}`, which is the
///   content change this model is for: a machine with claude *and* codex
///   installed used to fold both into one entry's prose and now advertises two
///   placed agents.
///
/// Seeds are equal to hand-written declarations in every other respect. They are
/// deliberately *not* written to disk here — the caller decides whether this is
/// a preview or a migration to persist — so a read-only run still produces a
/// full roster.
///
/// `harnesses` is the detected list in the daemon's own order; duplicates and
/// blanks are dropped. `default_harness` leads the result and is included even
/// when the detected list omits it, because the daemon will serve a task with it
/// either way.
pub fn seed_declarations(
    host_id: &str,
    workspace: &str,
    harnesses: &[&str],
    default_harness: &str,
) -> Vec<AgentDeclaration> {
    let default = default_harness.trim();
    let mut ordered: Vec<&str> = Vec::new();
    for harness in std::iter::once(&default)
        .chain(harnesses.iter())
        .map(|harness| harness.trim())
        .filter(|harness| !harness.is_empty())
    {
        if !ordered.contains(&harness) {
            ordered.push(harness);
        }
    }
    ordered
        .into_iter()
        .map(|harness| {
            let agent_id = if harness == default {
                host_id.to_string()
            } else {
                format!("{host_id}-{harness}")
            };
            AgentDeclaration::new(agent_id, host_id, harness, workspace)
        })
        .collect()
}

/// A stable, readable, unused `agentId` for a newly declared agent.
///
/// For the create-agent flow, where the operator has picked a harness and a
/// directory but should not have to invent an identifier. The directory name
/// carries the meaning (`medulla-claude`) because that is how a person refers to
/// the agent; a numeric suffix disambiguates a second agent of the same harness
/// in the same-named directory (two checkouts of one repo, which is ordinary).
///
/// `taken` is every id already declared anywhere in the fleet — ids are the
/// dispatch target, so a collision would silently route work to the wrong agent.
pub fn suggest_agent_id(workspace: &str, harness: &str, taken: &[String]) -> String {
    let folder = std::path::Path::new(workspace.trim())
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default();
    let base = match (slug(&folder).as_str(), slug(harness).as_str()) {
        ("", "") => "agent".to_string(),
        ("", harness) => harness.to_string(),
        (folder, "") => folder.to_string(),
        (folder, harness) => format!("{folder}-{harness}"),
    };
    if !taken.iter().any(|held| held == &base) {
        return base;
    }
    (2..)
        .map(|n| format!("{base}-{n}"))
        .find(|candidate| !taken.iter().any(|held| held == candidate))
        .expect("an unbounded search always terminates")
}

/// Lowercase, hyphen-separated, alphanumeric — safe to type and to round-trip.
fn slug(text: &str) -> String {
    let mut out = String::new();
    for c in text.chars() {
        if c.is_ascii_alphanumeric() {
            out.extend(c.to_lowercase());
        } else if !out.ends_with('-') && !out.is_empty() {
            out.push('-');
        }
    }
    out.trim_matches('-').to_string()
}
