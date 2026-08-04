//! The `Runtime` trait the UI drives, plus its snapshot contract. Concrete
//! implementations live alongside: [`openhuman`] (the embedded core, which the
//! product runs on) and [`mock`] (tests and demos). The UI depends only on the
//! trait and its types.
//!
//! The HTTP/SSE cloud-backend runtime and the unix-socket `medulla-serve`
//! runtime both lived here until the core was embedded; neither had a caller
//! afterwards.

pub mod capabilities;
mod event_log;
/// The declared-capacity containment chain and agent-template catalog.
pub mod fleet;
/// The non-interactive one-instruction driver for scripting / e2e automation.
pub mod headless;
pub mod mock;
pub mod openhuman;

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
    /// Compact symbol suitable for status bars.
    pub fn glyph(self) -> char {
        match self {
            StreamState::Live => '●',
            StreamState::Resyncing => '◌',
            StreamState::Stalled => '✕',
        }
    }
    /// Stable human-readable state label.
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
    ///
    /// Required rather than defaulted: the default was `"mock (scripted)"`, so
    /// an impl that forgot to override it told the operator their real runtime
    /// was a scripted demo. A wrong answer here is worse than no answer, and the
    /// compiler can insist on one.
    fn describe(&self) -> String;
    /// Account-level usage from the backend, when this runtime has one.
    /// `Ok(None)` = not supported by this runtime.
    fn team_usage(&self) -> BoxFuture<'static, anyhow::Result<Option<serde_json::Value>>> {
        Box::pin(std::future::ready(Ok(None)))
    }
    /// Return the current UI-facing state snapshot.
    fn snapshot(&self) -> RuntimeSnapshot;
    /// A change notification channel — a ping fires after every event/mutation.
    fn subscribe(&self) -> broadcast::Receiver<()>;
    /// Submit one user instruction to the active session.
    fn submit(&self, input: String) -> BoxFuture<'static, anyhow::Result<()>>;
    /// Whether a resolved [`submit`](Runtime::submit) means the cycle finished.
    ///
    /// True for every wire that answers only once the turn is done, which is
    /// what lets a caller report completion the moment the future resolves.
    /// A runtime whose submit returns on *acceptance* — the reply arriving
    /// later over the event stream — answers false, so the UI says the work was
    /// accepted instead of claiming a completion that has not happened yet.
    fn submit_settles_cycle(&self) -> bool {
        true
    }
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
    /// Request cancellation of the active cycle.
    fn abort(&self);
    /// Forget this host's stored session, so the next start asks for a sign-in.
    ///
    /// The default reports that there is nothing to log out of, which is the
    /// honest answer for a runtime that holds no credential of its own (the
    /// mock, a socket attachment). Only a runtime backed by a credential store
    /// overrides it — and it is the runtime's job rather than the UI's precisely
    /// because the UI has no way to know where the session lives.
    fn logout(&self) -> BoxFuture<'static, anyhow::Result<()>> {
        Box::pin(std::future::ready(Err(anyhow::anyhow!(
            "this runtime holds no session to log out of"
        ))))
    }
    /// Reset the runtime to a new main session.
    fn new_session(&self);
    /// Select the chat thread that receives subsequent input.
    fn set_active_thread(&self, id: String);
    /// List resumable top-level chats.
    fn list_main_chats(&self) -> BoxFuture<'static, anyhow::Result<Vec<MainChatSummary>>>;
    /// Resume a persisted top-level chat and its thread tree.
    fn resume_chat(&self, main_session_id: String) -> BoxFuture<'static, anyhow::Result<()>>;
    /// Return the context chunks currently visible to the model.
    fn inspect_context(&self) -> BoxFuture<'static, anyhow::Result<Vec<ContextItem>>>;
    /// Flush state and stop background runtime work.
    fn shutdown(&self) -> BoxFuture<'static, anyhow::Result<()>>;

    // --- operator steering & fleet ops (additive; core runtime only) -------------
    // Default no-ops so `MockRuntime` / `BackendRuntime` are unaffected — only the
    // core runtime, which speaks the worker.* / task.cancel / question.answer wire,
    // overrides them.

    /// Answer a pending `task_attention` question (`question.answer`). Fire-and-forget,
    /// like [`abort`](Runtime::abort).
    fn answer_question(&self, _cycle_id: String, _question_id: String, _body: String) {}

    /// Whether [`answer_question`](Runtime::answer_question) and
    /// [`cancel_task`](Runtime::cancel_task) actually reach the backend.
    ///
    /// Both are fire-and-forget and cannot report failure, so a runtime that
    /// inherits their no-op defaults would have the UI announce "Answer sent"
    /// over a question that stays blocked forever. This is what a surface checks
    /// before claiming either happened. Defaults to `true` — the wires that
    /// model steering are the ones that carried this trait first.
    fn steering_reaches_backend(&self) -> bool {
        true
    }

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

    /// The worker screens this runtime's hub is currently holding. Empty when
    /// the runtime has no hub, or when nothing is being watched.
    fn worker_screens(&self) -> Vec<crate::hub::WatchedScreen> {
        Vec::new()
    }

    /// Start or stop streaming the screen of the session running `task_id` on
    /// `worker`.
    ///
    /// A no-op success without a hub, so a runtime that cannot watch anything
    /// is not something every caller has to special-case.
    fn watch_task(
        &self,
        _worker: String,
        _task_id: String,
        _watch: bool,
    ) -> BoxFuture<'static, anyhow::Result<()>> {
        Box::pin(async { Ok(()) })
    }

    /// Kill the harness serving `task_id` on `worker`.
    ///
    /// A no-op success without a hub. Interactive callers are responsible for
    /// confirming the destructive action before invoking this method.
    fn kill_task(
        &self,
        _worker: String,
        _task_id: String,
    ) -> BoxFuture<'static, anyhow::Result<()>> {
        Box::pin(async { Ok(()) })
    }

    /// Tell the orchestrator a harness has been handed back, with the brief.
    ///
    /// An **error** by default rather than a silent success, unlike
    /// [`watch_task`](Self::watch_task). The whole point of a handoff is that
    /// the orchestrator is *told*, so a runtime that cannot tell it has to say
    /// so — an operator who typed a note and got a cheerful acknowledgement,
    /// with the note going nowhere, is the exact failure this feature exists to
    /// remove.
    fn hand_off_harness(
        &self,
        _brief: crate::hub::HarnessHandoff,
    ) -> BoxFuture<'static, anyhow::Result<()>> {
        Box::pin(async {
            Err(anyhow::anyhow!(
                "no hub is connected, so no handoff can be sent"
            ))
        })
    }

    /// Tell the orchestrator the operator has taken the harness in `workspace`.
    ///
    /// Errors for the same reason [`hand_off_harness`](Self::hand_off_harness)
    /// does: a takeover the backend never hears about leaves it dispatching into
    /// a workspace somebody is working in.
    fn hold_harness(
        &self,
        _workspace: String,
        _reason: Option<String>,
    ) -> BoxFuture<'static, anyhow::Result<()>> {
        Box::pin(async {
            Err(anyhow::anyhow!(
                "no hub is connected, so no hold can be sent"
            ))
        })
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

    // --- feedback board (additive; backend-backed runtimes only) -----------
    // The board lives on the cloud backend, so only the runtimes that hold a
    // backend client override these. `list_feedback` returning `Ok(None)` means
    // "this runtime has no board", which the UI renders as a sign-in hint
    // rather than an empty list; the mutating calls fail loudly for that case.

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
    demo_agents, demo_capacity, demo_fleet_requested, demo_requested_from, seed_declarations,
    suggest_agent_id, AgentDeclaration, AgentPlacement, AgentTemplate,
    AgentTemplateHarnessOverride, CapacitySnapshot, HarnessBudget, HarnessDescriptor,
    HostDescriptor, HostResources, WorkspaceDescriptor, WorkspaceProfile, WorkspaceRef,
    WorkspaceStrategy, DEMO_FLEET_ENV, SELECTABLE_STRATEGIES, WORKSPACE_TYPE_CHECKOUT,
    WORKTREE_MAX_SESSIONS,
};
pub use types::AgentDescriptor;
pub use types::AgentPresence;
pub use types::ContextItem;
pub use types::CycleResultSummary;
pub use types::LinkIdentity;
pub use types::PeerSession;
pub use types::RuntimeSnapshot;
pub use types::StreamState;
pub use types::SubmitReceipt;
pub use types::ThreadSummary;
pub use types::WorkerInfo;
pub use types::WorkerOp;
pub use types::{RoutingStrategy, SubscriptionRoutingStrategy};
