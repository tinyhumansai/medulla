//! The `Runtime` trait the UI drives, plus its snapshot contract. Concrete
//! implementations live alongside: [`backend`] (HTTP/SSE), [`core`] (the
//! core-js Unix socket, via [`core_client`]), and [`mock`] for tests and demos.
//! The UI depends only on the trait and its types.

pub mod backend;
pub mod core;
pub mod core_client;
pub mod mock;

use std::collections::HashMap;

use futures::future::BoxFuture;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use tokio::sync::broadcast;

use crate::ui::chat_store::{ChatMessage, MainChatSummary};
use crate::ui::events::{EventEnvelope, TaskDigest};

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
    /// Human-readable description of what backs this runtime, for the Overview.
    fn describe(&self) -> String {
        "mock (scripted)".into()
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
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runtime::mock::MockRuntime;

    #[test]
    fn stream_state_glyph_and_label() {
        assert_eq!(StreamState::Live.glyph(), '●');
        assert_eq!(StreamState::Resyncing.glyph(), '◌');
        assert_eq!(StreamState::Stalled.glyph(), '✕');
        assert_eq!(StreamState::Live.label(), "live");
        assert_eq!(StreamState::Resyncing.label(), "resyncing");
        assert_eq!(StreamState::Stalled.label(), "stalled");
    }

    #[test]
    fn stream_state_is_copy_and_eq() {
        let a = StreamState::Live;
        let b = a; // Copy
        assert_eq!(a, b);
        assert_ne!(StreamState::Live, StreamState::Stalled);
    }

    /// The trait's default methods are exercised through `MockRuntime`, which does
    /// not override any of them: they are the no-op fleet/steering seams.
    #[tokio::test]
    async fn default_trait_methods_are_no_ops() {
        let rt = MockRuntime::empty();
        // Fire-and-forget defaults: must not panic.
        rt.answer_question("cyc-1".into(), "q1".into(), "yes".into());
        rt.cancel_task("cyc-1".into(), "t1".into());
        // Default worker surface is empty and mutations succeed silently.
        assert!(rt.workers().is_empty());
        rt.worker_op(WorkerOp::Select { id: "w1".into() })
            .await
            .unwrap();
        rt.worker_op(WorkerOp::Add {
            address: Some("host:1".into()),
            handle: None,
            label: Some("lbl".into()),
            harness: None,
        })
        .await
        .unwrap();
        rt.worker_op(WorkerOp::Update {
            id: "w1".into(),
            patch: Map::new(),
        })
        .await
        .unwrap();
        rt.worker_op(WorkerOp::Remove { id: "w1".into() })
            .await
            .unwrap();
        // No lossy stream to surface.
        assert!(rt.stream_state().is_none());
    }

    #[test]
    fn agent_descriptor_serde_defaults() {
        // Only `id` is required; every other field defaults.
        let a: AgentDescriptor = serde_json::from_str(r#"{"id":"dev"}"#).unwrap();
        assert_eq!(a.id, "dev");
        assert!(a.name.is_empty());
        assert!(a.tags.is_empty());
        assert!(a.metadata.is_empty());
        let round: AgentDescriptor =
            serde_json::from_str(&serde_json::to_string(&a).unwrap()).unwrap();
        assert_eq!(a, round);
    }

    #[test]
    fn value_types_are_debug_clone_eq() {
        let presence = AgentPresence {
            online: true,
            detail: Some("idle".into()),
            at: 5,
        };
        assert_eq!(presence.clone(), presence);
        assert!(format!("{presence:?}").contains("AgentPresence"));

        let peer = PeerSession {
            id: "s1".into(),
            state: "idle".into(),
            harness: None,
            last_seen_at: 1,
        };
        assert_eq!(peer.clone(), peer);

        let thread = ThreadSummary {
            id: "t1".into(),
            parent_id: None,
            name: "main".into(),
            running: false,
            turns: 2,
            running_tasks: 0,
            attention: 0,
        };
        assert_eq!(thread.clone(), thread);

        let ident = TinyplaceIdentity {
            agent_id: "a".into(),
            public_key: "pk".into(),
            handle: Some("@h".into()),
        };
        assert_eq!(ident.clone(), ident);

        let worker = WorkerInfo {
            id: "w1".into(),
            address: "host".into(),
            handle: None,
            label: None,
            harness: None,
            peer_id: None,
            selected: false,
        };
        assert_eq!(worker.clone(), worker);

        let ctx = ContextItem {
            ref_: "r".into(),
            kind: "memory".into(),
            bytes: 3,
            content: "c".into(),
        };
        assert_eq!(ctx.clone(), ctx);
    }

    #[test]
    fn cycle_result_summary_default_is_empty() {
        let s = CycleResultSummary::default();
        assert_eq!(s.pass_count, 0);
        assert!(s.task_ledger.is_empty());
    }

    #[test]
    fn worker_op_is_debug_clone() {
        let op = WorkerOp::Add {
            address: Some("h".into()),
            handle: Some("@a".into()),
            label: None,
            harness: Some("codex".into()),
        };
        let cloned = op.clone();
        assert!(format!("{cloned:?}").contains("Add"));
    }
}
