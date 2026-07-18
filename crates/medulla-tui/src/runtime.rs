//! The `Runtime` trait the UI drives, plus its snapshot contract. A concrete
//! backend implementation lands separately; the UI depends only on this trait
//! and the [`MockRuntime`](crate::mock_runtime::MockRuntime).

use std::collections::HashMap;

use futures::future::BoxFuture;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use tokio::sync::broadcast;

use crate::chat_store::{ChatMessage, MainChatSummary};
use crate::events::{EventEnvelope, TaskDigest};

/// A connected agent medulla can delegate to.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
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
#[derive(Debug, Clone, PartialEq)]
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
#[derive(Debug, Clone)]
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
}

/// The runtime the TUI drives. Snapshot/subscribe are synchronous; the rest is
/// async where it may touch the backend.
pub trait Runtime: Send + Sync {
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
}
