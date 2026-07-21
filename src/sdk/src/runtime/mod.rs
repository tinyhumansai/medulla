//! The `Runtime` trait the UI drives, plus its snapshot contract. Concrete
//! implementations live alongside: [`backend`] (HTTP/SSE), [`mock`] (tests and
//! demos), and [`core`] (the unix-socket `medulla-serve` attach, unix-only). The
//! UI depends only on the trait and its types.

pub mod backend;
/// The `medulla-serve` NDJSON socket runtime (attach-only, unix-only).
#[cfg(unix)]
pub mod core;
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

/// A connected agent medulla can delegate to.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct AgentDescriptor {
    pub id: String,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub availability: String,
    #[serde(default)]
    pub tags: Vec<String>,
    #[serde(default)]
    pub metadata: Map<String, Value>,
}

/// Latest liveness reading for one roster agent (tinyplace backend only).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct AgentPresence {
    pub online: bool,
    pub detail: Option<String>,
    pub at: i64,
}

/// One wrapper session on a peer machine, as shown in the Agents view.
#[derive(Debug, Clone, PartialEq)]
pub struct PeerSession {
    pub id: String,
    pub state: String,
    pub harness: Option<String>,
    pub last_seen_at: i64,
}

/// One row in the Chat-tab thread sidebar.
#[derive(Debug, Clone, PartialEq)]
pub struct ThreadSummary {
    pub id: String,
    pub parent_id: Option<String>,
    pub name: String,
    pub running: bool,
    pub turns: usize,
    pub running_tasks: usize,
    pub attention: usize,
}

/// This TUI's own tiny.place identity.
#[derive(Debug, Clone, PartialEq)]
pub struct TinyplaceIdentity {
    pub agent_id: String,
    pub public_key: String,
    pub handle: Option<String>,
}

/// The last cycle's result, as surfaced in the Overview tab.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct CycleResultSummary {
    pub pass_count: i64,
    pub task_ledger: HashMap<String, TaskDigest>,
}

/// One managed worker peer, projected from a `worker.list` entry. A worker is a
/// remote tiny.place peer the orchestrator can delegate to. §4.2 is load-bearing:
/// `id` is the registry's own stable handle (for select/edit/remove), `address` is
/// the messaging target, and `peer_id` (the wallet) is a separate field — never
/// merged.
#[derive(Debug, Clone, PartialEq)]
pub struct WorkerInfo {
    pub id: String,
    pub address: String,
    pub handle: Option<String>,
    pub label: Option<String>,
    pub harness: Option<String>,
    pub peer_id: Option<String>,
    pub selected: bool,
}

/// A mutation on the worker-peer registry (`worker.add`/`select`/`update`/`remove`).
#[derive(Debug, Clone)]
pub enum WorkerOp {
    Add {
        address: Option<String>,
        handle: Option<String>,
        label: Option<String>,
        harness: Option<String>,
    },
    Select {
        id: String,
    },
    /// `patch` is a JSON object of the fields to change (e.g. `{"label": "..."}`); an
    /// empty-string value clears an optional field, mirroring `worker.update`.
    Update {
        id: String,
        patch: Map<String, Value>,
    },
    Remove {
        id: String,
    },
}

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

/// The event stream's health, surfaced in the header when a cycle runs under the
/// core runtime (§01 "lossy-but-not-silently").
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StreamState {
    /// `seq` is contiguous; the go-forward tap is trusted.
    Live,
    /// A `seq` gap was seen; a `snapshot.get` rebaselined the folded views.
    Resyncing,
    /// The stream has produced nothing for too long while a cycle is still in flight.
    Stalled,
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

/// One inspected context chunk (`inspect_context`).
#[derive(Debug, Clone, PartialEq)]
pub struct ContextItem {
    pub ref_: String,
    pub kind: String,
    pub bytes: usize,
    pub content: String,
}

/// The full render snapshot (see spec 01 appendix).
#[derive(Debug, Clone, Default)]
pub struct RuntimeSnapshot {
    pub session_id: String,
    pub running: bool,
    pub events: Vec<EventEnvelope>,
    pub chat_events: Vec<EventEnvelope>,
    pub messages: Vec<ChatMessage>,
    pub last_result: Option<CycleResultSummary>,
    pub tracing: bool,
    pub roster: Vec<AgentDescriptor>,
    pub presence: HashMap<String, AgentPresence>,
    pub sessions: HashMap<String, Vec<PeerSession>>,
    pub tinyplace: Option<TinyplaceIdentity>,
    pub async_mode: bool,
    pub threads: Vec<ThreadSummary>,
    pub active_thread_id: String,
    /// Latest agent-harness status, when the backing runtime fronts a medulla-v1
    /// agent harness. `None` until (and unless) the backend surfaces one; the
    /// Agents view renders the compact task board only while it is `Some`.
    pub harness: Option<crate::harness_contract::HarnessStatus>,
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
    fn abort(&self);
    fn new_session(&self);
    /// Fork the active thread, inheriting its history but with a fresh session.
    /// Returns the new thread id.
    fn fork(&self, name: Option<String>) -> String;
    fn set_active_thread(&self, id: String);
    fn list_main_chats(&self) -> BoxFuture<'static, anyhow::Result<Vec<MainChatSummary>>>;
    fn resume_chat(&self, main_session_id: String) -> BoxFuture<'static, anyhow::Result<()>>;
    fn set_async_mode(&self, on: bool) -> bool;
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

    /// The managed worker-peer registry snapshot (`worker.list`). Empty when the
    /// runtime has no worker surface.
    fn workers(&self) -> Vec<WorkerInfo> {
        Vec::new()
    }

    /// Apply a worker-registry mutation (`worker.*`). A no-op success elsewhere.
    fn worker_op(&self, _op: WorkerOp) -> BoxFuture<'static, anyhow::Result<()>> {
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
