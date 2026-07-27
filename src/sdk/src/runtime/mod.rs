//! The `Runtime` trait the UI drives, plus its snapshot contract. Concrete
//! implementations live alongside: [`backend`] (HTTP/SSE), [`mock`] (tests and
//! demos), and [`core`] (the unix-socket `medulla-serve` attach, unix-only). The
//! UI depends only on the trait and its types.

pub mod backend;
pub mod capabilities;
/// The `medulla-serve` NDJSON socket runtime (attach-only, unix-only).
#[cfg(unix)]
pub mod core;
mod event_log;
/// The declared-capacity containment chain and agent-template catalog.
pub mod fleet;
/// The non-interactive one-instruction driver for scripting / e2e automation.
pub mod headless;
pub mod mock;

use std::collections::HashMap;

use futures::future::BoxFuture;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use tokio::sync::broadcast;

use crate::client::{
    FeedbackComment, FeedbackDetail, FeedbackItem, FeedbackPage, FeedbackQuery, FeedbackSubmission,
    FeedbackType,
};
use crate::ui::chat_store::{ChatMessage, MainChatSummary};
use crate::ui::events::{EventEnvelope, TaskDigest};

impl WorkerOp {
    /// Parse a free-text "add worker" line into a [`WorkerOp::Add`].
    ///
    /// The first whitespace-delimited token is the identity; any remainder is a
    /// human label. A leading `@` marks the token as a tiny.place handle
    /// (`handle`); otherwise it is treated as an address. `harness` is left
    /// `None`. Returns `None` when `input` is blank so callers can surface an
    /// "empty" notice rather than issuing a no-op mutation.
    pub fn parse_add(input: &str) -> Option<Self> {
        let text = input.trim();
        if text.is_empty() {
            return None;
        }
        let (first, rest) = match text.split_once(char::is_whitespace) {
            Some((a, r)) => (a.trim().to_string(), r.trim().to_string()),
            None => (text.to_string(), String::new()),
        };
        let label = if rest.is_empty() { None } else { Some(rest) };
        let (address, handle) = if first.starts_with('@') {
            (None, Some(first))
        } else {
            (Some(first), None)
        };
        Some(WorkerOp::Add {
            address,
            handle,
            label,
            harness: None,
        })
    }
}

impl StreamState {
    pub fn glyph(self) -> char {
        match self {
            StreamState::Live => '●',
            StreamState::Resyncing => '◌',
            StreamState::Stalled => '✕',
        }
    }
    pub fn label(self) -> &'static str {
        match self {
            StreamState::Live => "live",
            StreamState::Resyncing => "resyncing",
            StreamState::Stalled => "stalled",
        }
    }
}

/// The runtime the TUI drives. Snapshot/subscribe are synchronous; the rest is
/// async where it may touch the backend.
pub trait Runtime: Send + Sync {
    /// Human-readable description of what backs this runtime, for the Overview.
    fn describe(&self) -> String {
        "mock (scripted)".into()
    }
    /// Account-level usage from the backend, when this runtime has one.
    /// `Ok(None)` = not supported by this runtime.
    fn team_usage(&self) -> BoxFuture<'static, anyhow::Result<Option<serde_json::Value>>> {
        Box::pin(std::future::ready(Ok(None)))
    }
    fn snapshot(&self) -> RuntimeSnapshot;
    /// A change notification channel — a ping fires after every event/mutation.
    fn subscribe(&self) -> broadcast::Receiver<()>;
    fn submit(&self, input: String) -> BoxFuture<'static, anyhow::Result<()>>;
    /// Like [`submit`](Runtime::submit), but returns the wire's correlation
    /// receipt when it carries one, so a caller waiting on the submitted
    /// cycle's end (the headless driver) can ignore other cycles' ends. The
    /// default delegates to `submit` and reports no receipt — runtimes whose
    /// wire has no receipt (mock, HTTP backend) need not override it.
    fn submit_with_receipt(
        &self,
        input: String,
    ) -> BoxFuture<'static, anyhow::Result<Option<SubmitReceipt>>> {
        let fut = self.submit(input);
        Box::pin(async move { fut.await.map(|_| None) })
    }
    fn abort(&self);
    fn new_session(&self);
    fn set_active_thread(&self, id: String);
    fn list_main_chats(&self) -> BoxFuture<'static, anyhow::Result<Vec<MainChatSummary>>>;
    fn resume_chat(&self, main_session_id: String) -> BoxFuture<'static, anyhow::Result<()>>;
    fn inspect_context(&self) -> BoxFuture<'static, anyhow::Result<Vec<ContextItem>>>;
    fn shutdown(&self) -> BoxFuture<'static, anyhow::Result<()>>;

    // --- operator steering & fleet ops (additive; core runtime only) -------------
    // Default no-ops so `MockRuntime` / `BackendRuntime` are unaffected — only the
    // core runtime, which speaks the worker.* / task.cancel / question.answer wire,
    // overrides them.

    /// Answer a pending `task_attention` question (`question.answer`). Fire-and-forget,
    /// like [`abort`](Runtime::abort).
    fn answer_question(&self, _cycle_id: String, _question_id: String, _body: String) {}

    /// Cancel a running task lane (`task.cancel`). Fire-and-forget.
    fn cancel_task(&self, _cycle_id: String, _task_id: String) {}

    /// What the managed workers are doing right now.
    ///
    /// Separate from the render snapshot's event log on purpose: that log is
    /// filled from the backend's SSE stream, whose vocabulary carries nothing
    /// about delegated tasks. A runtime that dispatches work itself knows, and
    /// this is how it says so. Empty when the runtime has no worker surface.
    fn worker_activity(&self) -> Vec<crate::hub::WorkerActivity> {
        Vec::new()
    }

    /// The managed worker-peer registry snapshot (`worker.list`). Empty when the
    /// runtime has no worker surface.
    fn workers(&self) -> Vec<WorkerInfo> {
        Vec::new()
    }

    /// Apply a worker-registry mutation (`worker.*`). A no-op success elsewhere.
    fn worker_op(&self, _op: WorkerOp) -> BoxFuture<'static, anyhow::Result<()>> {
        Box::pin(async { Ok(()) })
    }

    /// Re-read the declared fleet — the connected roster and the capacity chain
    /// it sits in — from the backing service, updating what the next
    /// [`snapshot`](Runtime::snapshot) reports.
    ///
    /// Pull rather than push because capacity is declared, not streamed: no
    /// runtime today emits an event when a host appears. A no-op success on
    /// runtimes whose capacity is fixed at attach time (core, mock), so callers
    /// may poll it unconditionally.
    fn refresh_fleet(&self) -> BoxFuture<'static, anyhow::Result<()>> {
        Box::pin(async { Ok(()) })
    }

    /// The event stream's health, when this runtime tracks one. `None` for runtimes
    /// with no lossy stream to surface (mock / HTTP backend).
    fn stream_state(&self) -> Option<StreamState> {
        None
    }

    // --- persona memory (additive; core runtime with an attached service) --------
    // Default: no memory surface. The core runtime overrides these from its
    // attached `MemoryService`; the mock runtime serves scripted values.

    /// The persona-memory health snapshot, when a memory service is attached.
    /// `None` when memory is disabled / not wired.
    fn memory_status(&self) -> Option<crate::memory::MemoryStatus> {
        None
    }

    /// Rank the persona corpus against `query`. Empty when no memory service is
    /// attached. `facet` is a loose facet name; unrecognized facets are ignored.
    fn memory_search(
        &self,
        _query: String,
        _facet: Option<String>,
        _k: usize,
    ) -> Vec<crate::memory::MemoryHit> {
        Vec::new()
    }

    /// The verbatim persona directives, when a memory service is attached.
    fn memory_directives(&self) -> Vec<String> {
        Vec::new()
    }

    // --- feedback board (additive; backend runtime only) -------------------
    // The board lives on the cloud backend, so only `BackendRuntime` overrides
    // these. `list_feedback` returning `Ok(None)` means "this runtime has no
    // board", which the UI renders as a sign-in hint rather than an empty list;
    // the mutating calls fail loudly for the same case.

    /// A page of the public feedback board. `Ok(None)` = this runtime has no
    /// backend to serve one.
    fn list_feedback(
        &self,
        _query: FeedbackQuery,
    ) -> BoxFuture<'static, anyhow::Result<Option<FeedbackPage>>> {
        Box::pin(std::future::ready(Ok(None)))
    }

    /// One board item with its comments.
    fn feedback_detail(&self, _id: String) -> BoxFuture<'static, anyhow::Result<FeedbackDetail>> {
        Box::pin(std::future::ready(Err(no_feedback_backend())))
    }

    /// Cast, change, or retract a vote (`1`, `-1`, `0`). Returns the item with
    /// recomputed tallies.
    fn vote_feedback(
        &self,
        _id: String,
        _value: i8,
    ) -> BoxFuture<'static, anyhow::Result<FeedbackItem>> {
        Box::pin(std::future::ready(Err(no_feedback_backend())))
    }

    /// Post a comment on a board item.
    fn comment_feedback(
        &self,
        _id: String,
        _body: String,
    ) -> BoxFuture<'static, anyhow::Result<FeedbackComment>> {
        Box::pin(std::future::ready(Err(no_feedback_backend())))
    }

    /// Submit new feedback. A moderation rejection is a successful call with
    /// [`FeedbackSubmission::accepted`] false — not an error.
    fn submit_feedback(
        &self,
        _kind: FeedbackType,
        _title: String,
        _body: String,
    ) -> BoxFuture<'static, anyhow::Result<FeedbackSubmission>> {
        Box::pin(std::future::ready(Err(no_feedback_backend())))
    }
}

/// The error every feedback mutation returns on a runtime with no backend.
fn no_feedback_backend() -> anyhow::Error {
    anyhow::anyhow!("the feedback board requires a signed-in backend connection")
}

#[cfg(test)]
mod tests;

mod types;
pub use fleet::{
    demo_agents, demo_capacity, demo_fleet_requested, demo_requested_from, AgentPlacement,
    AgentTemplate, AgentTemplateHarnessOverride, CapacitySnapshot, HarnessBudget,
    HarnessDescriptor, HostDescriptor, HostResources, WorkspaceDescriptor, WorkspaceProfile,
    DEMO_FLEET_ENV,
};
pub use types::AgentDescriptor;
pub use types::AgentPresence;
pub use types::ContextItem;
pub use types::CycleResultSummary;
pub use types::PeerSession;
pub use types::RuntimeSnapshot;
pub use types::StreamState;
pub use types::SubmitReceipt;
pub use types::ThreadSummary;
pub use types::TinyplaceIdentity;
pub use types::WorkerInfo;
pub use types::WorkerOp;
pub use types::{RoutingStrategy, SubscriptionRoutingStrategy};
