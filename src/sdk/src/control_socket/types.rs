//! Wire types for the control protocol, and the [`FleetOps`] seam the pure
//! request handler is written against.
//!
//! Keeping the fleet behind a trait is what lets the handler be tested without a
//! hub, a socket, or a harness: a fake implementation answers every branch.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};
use tokio::sync::mpsc;

use crate::hub::{RunError, TaskOutcome, TaskRequest};

/// The control protocol version this build speaks.
///
/// Bumped when a frame's shape changes incompatibly. The shim and the server are
/// the same binary in every supported deployment — Medulla spawns its own MCP
/// subprocess — so a mismatch means a stale process survived an upgrade, and
/// saying so beats guessing.
pub const PROTOCOL_VERSION: u32 = 1;

/// Why a control request failed.
///
/// A machine-readable slug rather than a bare string because the tool layer maps
/// several of these to advice a model can act on: `hubNotReady` means "ask
/// again", `depthExceeded` means "stop and do the work yourself", and
/// `tooManyInFlight` means "wait for one to finish". A caller that cannot tell
/// them apart either gives up on a transient refusal or hammers a permanent one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum ErrorKind {
    /// No grant was presented, or the one presented is not (or no longer) valid.
    Unauthenticated,
    /// The peer speaks a control protocol this build does not.
    VersionMismatch,
    /// The hub has not finished connecting, so the roster is not yet knowable.
    ///
    /// Deliberately distinct from an empty roster: a caller told "no workers"
    /// concludes the fleet is unusable, where this one means "ask again".
    HubNotReady,
    /// The named worker is not in the roster.
    NoSuchWorker,
    /// No task with that id was dispatched under this grant.
    NoSuchTask,
    /// The holder is already as deep in a dispatch tree as it may go.
    DepthExceeded,
    /// The holder already has its maximum concurrent dispatches running.
    TooManyInFlight,
    /// The dispatch was attempted and the fleet refused or failed it.
    DispatchFailed,
    /// The frame was not understood, or a required parameter was missing.
    BadRequest,
    /// Something failed on this side that the caller cannot act on.
    Internal,
}

impl ErrorKind {
    /// The slug as it appears on the wire.
    pub fn as_wire(self) -> &'static str {
        match self {
            ErrorKind::Unauthenticated => "unauthenticated",
            ErrorKind::VersionMismatch => "versionMismatch",
            ErrorKind::HubNotReady => "hubNotReady",
            ErrorKind::NoSuchWorker => "noSuchWorker",
            ErrorKind::NoSuchTask => "noSuchTask",
            ErrorKind::DepthExceeded => "depthExceeded",
            ErrorKind::TooManyInFlight => "tooManyInFlight",
            ErrorKind::DispatchFailed => "dispatchFailed",
            ErrorKind::BadRequest => "badRequest",
            ErrorKind::Internal => "internal",
        }
    }

    /// Whether the same request a little later might succeed.
    ///
    /// Carried to the model so it can tell "nothing was attempted, try again"
    /// from "this will never work".
    pub fn is_retryable(self) -> bool {
        matches!(
            self,
            ErrorKind::HubNotReady | ErrorKind::TooManyInFlight | ErrorKind::DispatchFailed
        )
    }
}

/// A refusal, as the server returns it and the client surfaces it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ControlFailure {
    /// What kind of refusal this is.
    pub kind: ErrorKind,
    /// The sentence a model reads. Written to say what to do instead, not just
    /// what went wrong.
    pub message: String,
}

impl ControlFailure {
    /// Build a failure of `kind` with `message`.
    pub fn new(kind: ErrorKind, message: impl Into<String>) -> Self {
        ControlFailure {
            kind,
            message: message.into(),
        }
    }
}

impl std::fmt::Display for ControlFailure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.kind.as_wire(), self.message)
    }
}

/// Why a client-side control call did not produce an answer.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum ControlError {
    /// No Medulla instance is listening on the configured socket.
    ///
    /// The ordinary case for a harness whose spawn carried no grant — a remote
    /// worker, or a host with fleet tools turned off — and never an error worth
    /// failing a session over.
    NoInstance,
    /// The connection dropped, possibly mid-call.
    Disconnected(String),
    /// The server answered with a refusal.
    Refused(ControlFailure),
    /// The transport itself failed.
    Transport(String),
}

impl std::fmt::Display for ControlError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ControlError::NoInstance => {
                write!(f, "no running Medulla instance is reachable from here")
            }
            ControlError::Disconnected(m) => write!(f, "control connection dropped: {m}"),
            ControlError::Refused(failure) => write!(f, "{failure}"),
            ControlError::Transport(m) => write!(f, "control transport error: {m}"),
        }
    }
}

impl std::error::Error for ControlError {}

/// Which families of tool a grant may call.
///
/// Two orthogonal switches rather than an enum of combinations, so adding a
/// third family later does not multiply the variants. Both default on: a
/// harness Medulla spawned for ordinary work gets the whole surface, and the
/// narrowing cases (a review turn, a depth-capped child) are the exceptions.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolFamilies {
    /// The `workflow_*` tools: author, validate, and run saved graphs.
    pub workflows: bool,
    /// The `fleet_*` tools: see the fleet and dispatch work into it.
    pub fleet: bool,
}

impl Default for ToolFamilies {
    fn default() -> Self {
        ToolFamilies {
            workflows: true,
            fleet: true,
        }
    }
}

impl ToolFamilies {
    /// Only the workflow tools — what a harness with no fleet grant is served.
    pub fn workflows_only() -> Self {
        ToolFamilies {
            workflows: true,
            fleet: false,
        }
    }

    /// Only the fleet tools — what a fleet host serves when workflows are off.
    pub fn fleet_only() -> Self {
        ToolFamilies {
            workflows: false,
            fleet: true,
        }
    }

    /// Whether a tool by this name belongs to a family this grant may call.
    ///
    /// Matched on the name's prefix, which is why the prefixes are load-bearing
    /// rather than decorative. A name in neither family is allowed through: the
    /// families gate the two surfaces that exist, and a future tool that fits
    /// neither should not be silently withheld by a check that predates it.
    pub fn allows(self, name: &str) -> bool {
        if name.starts_with("fleet_") {
            return self.fleet;
        }
        if name.starts_with("workflow_") {
            return self.workflows;
        }
        true
    }
}

/// One worker in the fleet, as a tool caller sees it.
///
/// A projection of [`crate::hub::HubWorker`] rather than the type itself: the
/// roster entry carries transport details (public keys, handoff briefs) that a
/// model has no use for and should not have to reason about.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct FleetWorker {
    /// The id to name when dispatching.
    pub id: String,
    /// The worker's bridge address.
    pub address: String,
    /// The coding harness it runs.
    pub harness: String,
    /// A human label, when the operator set one.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    /// Agent-template ids this worker is offered for. Empty means unspecified.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub roles: Vec<String>,
    /// The directory it runs tasks in, when known.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub workspace: Option<String>,
    /// Whether this is the fleet's currently-selected default worker.
    pub selected: bool,
    /// Whether a person currently holds this worker's harness.
    ///
    /// A held worker refuses dispatches, so this is the difference between
    /// routing work somewhere it will run and somewhere it will bounce.
    pub held: bool,
    /// Why they hold it, when they said.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub held_reason: Option<String>,
    /// Ids of the tasks it is running right now.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub running: Vec<String>,
}

/// The live fleet, as the control plane needs it.
///
/// Implemented over a real [`crate::hub::HubHandle`] in production and over a
/// fake in tests. Deliberately *not*
/// [`HarnessDispatch`](crate::flow_engine::caps::HarnessDispatch): that trait
/// has no roster surface, its only cancellation is all-or-nothing, and it sits
/// behind the `workflows` feature which this module must not inherit.
#[async_trait::async_trait]
pub trait FleetOps: Send + Sync + 'static {
    /// The roster, or `None` while the hub is still connecting.
    ///
    /// The `Option` is the whole point: a caller handed an empty list at second
    /// zero concludes the fleet is empty and stops asking, where `None` maps to
    /// a refusal that says to try again.
    fn workers(&self) -> Option<Vec<FleetWorker>>;

    /// Where a dispatch goes when the caller named no worker.
    fn default_worker(&self) -> Option<String>;

    /// Run one task to completion.
    ///
    /// Long-running by nature — the hub owns no wall-clock deadline — so callers
    /// spawn this rather than awaiting it inline.
    async fn dispatch(
        &self,
        request: TaskRequest,
        status: Option<mpsc::UnboundedSender<String>>,
    ) -> Result<TaskOutcome, RunError>;

    /// Stop the dispatch registered under `abort_id`.
    ///
    /// Best-effort, matching [`crate::hub::TaskRunner::abort_task`]: a task that
    /// already settled is a no-op rather than an error.
    fn abort(&self, abort_id: &str);
}

/// What a successful `hello` tells the shim about the fleet it just reached.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Hello {
    /// The control protocol the server speaks.
    pub protocol: u32,
    /// The server binary's version.
    pub version: String,
    /// Whether the hub has connected and the roster is knowable.
    pub hub_ready: bool,
    /// How deep in the dispatch tree this grant's holder sits.
    pub depth: u8,
    /// The depth at which `fleet_dispatch` is withheld.
    pub max_depth: u8,
    /// The most concurrent dispatches this grant may hold.
    pub max_in_flight: usize,
    /// Which tool families this grant may call.
    pub families: ToolFamilies,
}

impl Hello {
    /// Whether this grant's holder may still dispatch.
    ///
    /// Read by the shim at startup to decide whether to advertise
    /// `fleet_dispatch` at all. Withholding the verb is the guard; the server's
    /// own check is the backstop.
    pub fn may_dispatch(&self) -> bool {
        self.families.fleet && self.depth < self.max_depth
    }
}

/// Environment variable naming the control socket a spawned harness may reach.
///
/// Set by Medulla on the MCP subprocess it spawns, never inherited from the
/// ambient environment of whoever started Medulla.
pub const MCP_SOCKET_ENV: &str = "MEDULLA_MCP_SOCKET";

/// Environment variable carrying the grant token for that socket.
///
/// The token is the authority: everything its holder may do is looked up
/// server-side from it, so a model that rewrites this value cannot widen its own
/// permissions, only present one that means nothing.
pub const MCP_GRANT_ENV: &str = "MEDULLA_MCP_GRANT";

/// Environment variable carrying a task's depth in the dispatch tree.
///
/// Set by the daemon from [`TaskRequest::fleet_depth`](crate::hub::TaskRequest),
/// and read when minting the grant for the harness that task runs on. Absent
/// means depth zero: work an operator started.
///
/// Not a security boundary on its own — a harness could rewrite it in its own
/// environment — which is why the value it produces is written into a *grant*
/// held server-side, and every later check reads the grant rather than the
/// environment.
pub const FLEET_DEPTH_ENV: &str = "MEDULLA_FLEET_DEPTH";

/// The depth this task runs at, from the environment its harness was given.
pub fn depth_from_env(env: &HashMap<String, String>) -> u8 {
    env.get(FLEET_DEPTH_ENV)
        .and_then(|value| value.trim().parse().ok())
        .unwrap_or(0)
}

/// Read the socket path and grant token a spawn was handed, if it was handed any.
///
/// `None` whenever either is missing, which is the ordinary state for a harness
/// running on a remote worker or on a host with fleet tools turned off.
pub fn grant_from_env(env: &HashMap<String, String>) -> Option<(std::path::PathBuf, String)> {
    let socket = env
        .get(MCP_SOCKET_ENV)
        .map(|s| s.trim())
        .filter(|s| !s.is_empty())?;
    let token = env
        .get(MCP_GRANT_ENV)
        .map(|s| s.trim())
        .filter(|s| !s.is_empty())?;
    Some((std::path::PathBuf::from(socket), token.to_string()))
}
