//! The store side of the workflow plane: the trait an embedding host installs.
//!
//! The transport half lives in `hub::workflows`; this is the contract it calls
//! into. Moved here with the payloads it speaks — see
//! [`super::payloads`] for why a Medulla-to-Medulla-backend contract stopped
//! being sourced from a desktop product.

use async_trait::async_trait;
use serde_json::Value;

use super::payloads::{CopilotOutcome, WorkflowDescriptor};

/// The store side of the workflow plane, supplied by the embedding host.
///
/// The reads are synchronous because every host backs them with a local store;
/// they are dispatched on a blocking thread so a slow store cannot stall the
/// socket runtime. `copilot` is `async` because it is not a read at all — it is
/// a whole agent turn on the host's own authoring copilot.
///
/// Every method returns `Result<_, String>` rather than a host error type: the
/// error is going straight onto a model's prompt surface as a tool result, so it
/// must already be a sentence a reader can act on.
#[async_trait]
pub trait WorkflowBridge: Send + Sync {
    /// The adverts this host publishes. Cheap and side-effect free — it is
    /// called on every (re)connect and on every store change.
    fn list(&self) -> Vec<WorkflowDescriptor>;

    /// Fallible advert read used by registration.
    ///
    /// The default preserves compatibility for hosts whose list cannot fail.
    /// Store-backed hosts override it so a transient read failure does not look
    /// like a successful empty replacement batch.
    fn try_list(&self) -> Result<Vec<WorkflowDescriptor>, String> {
        Ok(self.list())
    }

    /// One workflow's detail, graph included, as the host's own JSON shape.
    fn get(&self, id: &str) -> Result<Value, String>;

    /// The node-kind catalog, optionally narrowed to a single `kind`. This is an
    /// authoring aid: a manager reads it to brief the copilot accurately.
    fn node_kinds(&self, kind: Option<&str>) -> Result<Value, String>;

    /// Recent runs of one workflow, for visibility into a delegated graph.
    fn runs(&self, id: &str) -> Result<Value, String>;

    /// Run one authoring turn on the host's copilot. `workflow_id` absent means
    /// "create a new workflow". The outcome must be derived from a re-read of
    /// the host's own store rather than from the model's claim about what it
    /// did — that is the accountability property the whole delegation rests on.
    async fn copilot(
        &self,
        instruction: &str,
        workflow_id: Option<&str>,
    ) -> Result<CopilotOutcome, String>;

    /// Which roster agent owns these workflows, when the host runs more than
    /// one identity. Stamped onto the advert batch so the backend can route a
    /// delegated workflow to its owner. Defaults to unset — the backend then
    /// treats the whole socket as the owner.
    fn agent_id(&self) -> Option<String> {
        None
    }

    /// The action directory associated with this bridge's authenticated
    /// identity, when the host has one. Capability probes use this instead of
    /// reloading process-global active-user state.
    fn action_dir(&self) -> Option<String> {
        None
    }
}
