//! Data types for the harness socket client.

use std::sync::Arc;

use super::super::roster::{SharedRoster, SharedSubscriptionStrategy};
use super::super::runner::TaskRunner;
use super::super::types::HubLog;
use super::super::workflows::WorkflowPlane;
use super::super::ActivityLog;

/// Everything the harness client dispatches through, beyond the URL it dials.
///
/// A struct rather than a parameter list, for the same reason
/// [`HandleWiring`](super::super::handle::HandleWiring) is one: the client needs
/// the roster, the runner, two policies and two sinks, and a positional list
/// that long is a place where two arguments of the same type get silently
/// transposed.
pub(in super::super) struct HarnessWiring {
    /// The shared roster advertised on every (re)connect and resolved against on
    /// every task frame.
    pub roster: SharedRoster,
    /// Agent-role definitions used to decorate roster adverts and constrain
    /// capability replies.
    pub catalog: Arc<Vec<crate::runtime::AgentTemplate>>,
    /// Where a delegated task is dispatched.
    pub runner: Arc<TaskRunner>,
    /// How an untargeted task chooses among a worker's provider subscriptions.
    pub subscription_strategy: SharedSubscriptionStrategy,
    /// Where socket diagnostics are narrated.
    pub log: HubLog,
    /// What the workers are doing, for the Agents view. `None` keeps no history.
    pub activity: Option<ActivityLog>,
    /// The store side of the cloud workflow plane.
    ///
    /// `Some` makes this hub advertise its saved graphs (`medulla:register_workflows`)
    /// and serve the reads and the authoring turn the orchestrator round-trips
    /// back. `None` advertises nothing — but the request handler is still
    /// registered, because a request addressed here must be *refused*, not
    /// dropped: silence costs the backend the op's whole deadline.
    pub workflows: Option<WorkflowPlane>,
}
