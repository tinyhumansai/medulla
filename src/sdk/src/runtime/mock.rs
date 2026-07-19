//! A scripted, self-contained [`Runtime`] used by `main` until the backend
//! runtime lands, and by tests. It fabricates a plausible event stream so every
//! tab has something to render.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use futures::future::BoxFuture;
use serde_json::{json, Map};
use tokio::sync::broadcast;

use crate::runtime::{
    AgentDescriptor, AgentPresence, ContextItem, CycleResultSummary, PeerSession, Runtime,
    RuntimeSnapshot, ThreadSummary, TinyplaceIdentity,
};
use crate::ui::chat_store::{ChatMessage, MainChatSummary};
use crate::ui::events::{EventEnvelope, TaskDigest, TuiEvent, Usage};

const EVENT_CAP: usize = 5000;
const CHAT_CAP: usize = 2000;

struct Thread {
    id: String,
    parent_id: Option<String>,
    name: String,
    session_id: String,
    messages: Vec<ChatMessage>,
    events: Vec<EventEnvelope>,
    chat_events: Vec<EventEnvelope>,
    running: bool,
    last_result: Option<CycleResultSummary>,
}

struct State {
    threads: Vec<Thread>,
    active_id: String,
    seq: u64,
    cycle_seq: u64,
    async_mode: bool,
    tracing: bool,
    roster: Vec<AgentDescriptor>,
    presence: HashMap<String, AgentPresence>,
    sessions: HashMap<String, Vec<PeerSession>>,
    tinyplace: Option<TinyplaceIdentity>,
}

impl State {
    fn active_mut(&mut self) -> &mut Thread {
        let id = self.active_id.clone();
        self.threads
            .iter_mut()
            .find(|t| t.id == id)
            .expect("active thread")
    }

    fn active(&self) -> &Thread {
        self.threads
            .iter()
            .find(|t| t.id == self.active_id)
            .expect("active thread")
    }

    fn emit(&mut self, event: TuiEvent) {
        self.seq += 1;
        let env = EventEnvelope {
            seq: self.seq,
            at: now_millis(),
            event,
        };
        let chatty = matches!(
            env.event,
            TuiEvent::User { .. } | TuiEvent::Assistant { .. } | TuiEvent::Error { .. }
        );
        let thread = self.active_mut();
        thread.events.push(env.clone());
        if thread.events.len() > EVENT_CAP {
            let drop = thread.events.len() - EVENT_CAP;
            thread.events.drain(0..drop);
        }
        if chatty {
            thread.chat_events.push(env);
            if thread.chat_events.len() > CHAT_CAP {
                let drop = thread.chat_events.len() - CHAT_CAP;
                thread.chat_events.drain(0..drop);
            }
        }
    }
}

fn now_millis() -> i64 {
    crate::ui::chat_store::now_millis()
}

fn gen_id(prefix: &str) -> String {
    format!("{prefix}-{}-{:04x}", now_millis(), rand_suffix())
}

fn rand_suffix() -> u16 {
    // Cheap, dependency-free pseudo-random from the clock.
    (now_millis() as u64)
        .wrapping_mul(2654435761)
        .rotate_left(13) as u16
}

/// A scripted runtime. Construct with [`MockRuntime::demo`] for a populated
/// snapshot or [`MockRuntime::empty`] for a bare one.
pub struct MockRuntime {
    state: Arc<Mutex<State>>,
    tx: broadcast::Sender<()>,
    calls: Arc<Mutex<Vec<String>>>,
    /// Scripted persona-memory surface (test seam). `None` = no memory service.
    memory: Arc<Mutex<Option<ScriptedMemory>>>,
}

/// A scripted stand-in for a `MemoryService`, driven by tests via the
/// `set_memory_*` seams.
#[derive(Default, Clone)]
struct ScriptedMemory {
    status: Option<crate::memory::MemoryStatus>,
    hits: Vec<crate::memory::MemoryHit>,
    directives: Vec<String>,
}

impl MockRuntime {
    fn from_state(state: State) -> Self {
        let (tx, _rx) = broadcast::channel(256);
        MockRuntime {
            state: Arc::new(Mutex::new(state)),
            tx,
            calls: Arc::new(Mutex::new(Vec::new())),
            memory: Arc::new(Mutex::new(None)),
        }
    }

    /// Attach a scripted memory status. Enables the mock's memory surface.
    pub fn set_memory_status(&self, status: crate::memory::MemoryStatus) {
        let mut guard = self.memory.lock().unwrap();
        guard.get_or_insert_with(ScriptedMemory::default).status = Some(status);
    }

    /// Script the hits returned by [`Runtime::memory_search`].
    pub fn set_memory_hits(&self, hits: Vec<crate::memory::MemoryHit>) {
        let mut guard = self.memory.lock().unwrap();
        guard.get_or_insert_with(ScriptedMemory::default).hits = hits;
    }

    /// Script the directives returned by [`Runtime::memory_directives`].
    pub fn set_memory_directives(&self, directives: Vec<String>) {
        let mut guard = self.memory.lock().unwrap();
        guard.get_or_insert_with(ScriptedMemory::default).directives = directives;
    }

    fn record(&self, name: &str) {
        self.calls.lock().unwrap().push(name.to_string());
    }

    /// The ordered log of runtime methods invoked on this mock. Test seam.
    pub fn recorded_calls(&self) -> Vec<String> {
        self.calls.lock().unwrap().clone()
    }

    /// Emit an arbitrary event into the active thread and notify subscribers.
    /// Test/demo scripting seam.
    pub fn script_event(&self, event: TuiEvent) {
        {
            self.state.lock().unwrap().emit(event);
        }
        self.ping();
    }

    /// Force the active thread's running flag. Test/demo scripting seam.
    pub fn set_running(&self, running: bool) {
        {
            self.state.lock().unwrap().active_mut().running = running;
        }
        self.ping();
    }

    /// A bare runtime: one empty main thread, no roster.
    pub fn empty() -> Self {
        let session_id = gen_id("tui");
        let state = State {
            threads: vec![Thread {
                id: "t1".into(),
                parent_id: None,
                name: "main".into(),
                session_id,
                messages: Vec::new(),
                events: Vec::new(),
                chat_events: Vec::new(),
                running: false,
                last_result: None,
            }],
            active_id: "t1".into(),
            seq: 0,
            cycle_seq: 0,
            async_mode: false,
            tracing: false,
            roster: Vec::new(),
            presence: HashMap::new(),
            sessions: HashMap::new(),
            tinyplace: None,
        };
        MockRuntime::from_state(state)
    }

    /// A populated runtime: a roster, presence, a couple of turns and a
    /// completed delegated task, for a lively demo.
    pub fn demo() -> Self {
        let rt = MockRuntime::empty();
        {
            let mut s = rt.state.lock().unwrap();
            let mut meta = Map::new();
            meta.insert("harness".into(), json!("tinyplace"));
            meta.insert("handle".into(), json!("@dev-1"));
            meta.insert("protocol".into(), json!("openhuman"));
            s.roster = vec![AgentDescriptor {
                id: "dev-1".into(),
                name: "dev-1".into(),
                description: "A remote coding agent for delegated implementation work.".into(),
                availability: "online".into(),
                tags: vec!["code".into()],
                metadata: meta,
            }];
            s.presence.insert(
                "dev-1".into(),
                AgentPresence {
                    online: true,
                    detail: Some("idle".into()),
                    at: now_millis(),
                },
            );
            s.tinyplace = Some(TinyplaceIdentity {
                agent_id: "cid-abc123".into(),
                public_key: "pk".into(),
                handle: Some("@medulla".into()),
            });
            s.tracing = true;

            s.emit(TuiEvent::CycleStart {
                cycle_id: "cyc-1".into(),
            });
            s.emit(TuiEvent::User {
                body: "Summarize the repo and delegate a refactor.".into(),
            });
            s.active_mut().messages.push(ChatMessage {
                role: "user".into(),
                content: "Summarize the repo and delegate a refactor.".into(),
            });
            s.emit(TuiEvent::InferenceEnd {
                tier: "orchestrator".into(),
                op: "orchestrate".into(),
                model: Some("gpt-4o".into()),
                duration_ms: 820,
                usage: Some(Usage {
                    input_tokens: 1200,
                    output_tokens: 90,
                }),
                content: None,
                reasoning: None,
                tool_calls: None,
            });
            s.emit(TuiEvent::TaskStart {
                task_id: "task-1".into(),
                instruction: "Refactor the auth module for clarity.".into(),
                depth: 2,
                agent_id: Some("dev-1".into()),
            });
            s.emit(TuiEvent::TaskEvent {
                task_id: "task-1".into(),
                event_kind: "text".into(),
                content: "Reading auth module…".into(),
                harness: Some("CODEX".into()),
            });
            s.emit(TuiEvent::TaskComplete {
                digest: TaskDigest {
                    task_id: "task-1".into(),
                    status: "done".into(),
                    digest: "Refactored auth into 3 focused modules.".into(),
                    result_ref: None,
                    usage: Some(Usage {
                        input_tokens: 6400,
                        output_tokens: 420,
                    }),
                    depth: 2,
                },
            });
            let reply = "Done — I mapped the repo and delegated the auth refactor to dev-1.";
            s.emit(TuiEvent::Assistant { body: reply.into() });
            s.active_mut().messages.push(ChatMessage {
                role: "assistant".into(),
                content: reply.into(),
            });
            s.emit(TuiEvent::CycleEnd {
                cycle_id: "cyc-1".into(),
                pass_count: 2,
                duration_ms: 4200,
            });
            let mut ledger = HashMap::new();
            ledger.insert(
                "task-1".into(),
                TaskDigest {
                    task_id: "task-1".into(),
                    status: "done".into(),
                    digest: "Refactored auth into 3 focused modules.".into(),
                    result_ref: None,
                    usage: Some(Usage {
                        input_tokens: 6400,
                        output_tokens: 420,
                    }),
                    depth: 2,
                },
            );
            s.active_mut().last_result = Some(CycleResultSummary {
                pass_count: 2,
                task_ledger: ledger,
            });
            s.sessions.insert(
                "dev-1".into(),
                vec![PeerSession {
                    id: "sess-9".into(),
                    state: "idle".into(),
                    harness: Some("codex".into()),
                    last_seen_at: now_millis(),
                }],
            );
        }
        rt
    }

    fn ping(&self) {
        let _ = self.tx.send(());
    }

    fn thread_summaries(state: &State) -> Vec<ThreadSummary> {
        state
            .threads
            .iter()
            .map(|t| {
                let mut running_tasks = 0i64;
                let mut attention = 0usize;
                for env in &t.events {
                    match &env.event {
                        TuiEvent::TaskStart { .. } => running_tasks += 1,
                        TuiEvent::TaskComplete { .. } => running_tasks -= 1,
                        TuiEvent::TaskAttention { .. } | TuiEvent::Error { .. } => attention += 1,
                        _ => {}
                    }
                }
                ThreadSummary {
                    id: t.id.clone(),
                    parent_id: t.parent_id.clone(),
                    name: t.name.clone(),
                    running: t.running,
                    turns: t.messages.len().div_ceil(2),
                    running_tasks: running_tasks.max(0) as usize,
                    attention,
                }
            })
            .collect()
    }
}

impl Runtime for MockRuntime {
    fn snapshot(&self) -> RuntimeSnapshot {
        let s = self.state.lock().unwrap();
        let threads = Self::thread_summaries(&s);
        let active = s.active();
        RuntimeSnapshot {
            session_id: active.session_id.clone(),
            running: active.running,
            events: active.events.clone(),
            chat_events: active.chat_events.clone(),
            messages: active.messages.clone(),
            last_result: active.last_result.clone(),
            tracing: s.tracing,
            roster: s.roster.clone(),
            presence: s.presence.clone(),
            sessions: s.sessions.clone(),
            tinyplace: s.tinyplace.clone(),
            async_mode: s.async_mode,
            threads,
            active_thread_id: s.active_id.clone(),
        }
    }

    fn subscribe(&self) -> broadcast::Receiver<()> {
        self.tx.subscribe()
    }

    fn submit(&self, input: String) -> BoxFuture<'static, anyhow::Result<()>> {
        self.record("submit");
        let state = self.state.clone();
        let tx = self.tx.clone();
        Box::pin(async move {
            {
                let mut s = state.lock().unwrap();
                if s.active().running {
                    return Err(anyhow::anyhow!("a cycle is already running"));
                }
                s.cycle_seq += 1;
                let cid = format!("cyc-{}", s.cycle_seq);
                s.active_mut().running = true;
                s.emit(TuiEvent::CycleStart {
                    cycle_id: cid.clone(),
                });
                s.emit(TuiEvent::User {
                    body: input.clone(),
                });
                s.active_mut().messages.push(ChatMessage {
                    role: "user".into(),
                    content: input,
                });
                s.emit(TuiEvent::InferenceStart {
                    tier: "reasoning".into(),
                    op: "execute_step".into(),
                    model: Some("gpt-4o".into()),
                });
            }
            let _ = tx.send(());
            tokio::time::sleep(std::time::Duration::from_millis(500)).await;
            {
                let mut s = state.lock().unwrap();
                s.emit(TuiEvent::InferenceEnd {
                    tier: "reasoning".into(),
                    op: "execute_step".into(),
                    model: Some("gpt-4o".into()),
                    duration_ms: 500,
                    usage: Some(Usage {
                        input_tokens: 800,
                        output_tokens: 120,
                    }),
                    content: Some("Here is my reasoning.".into()),
                    reasoning: None,
                    tool_calls: None,
                });
                let reply = "(mock) I processed your request.".to_string();
                s.emit(TuiEvent::Assistant {
                    body: reply.clone(),
                });
                s.active_mut().messages.push(ChatMessage {
                    role: "assistant".into(),
                    content: reply,
                });
                let cid = format!("cyc-{}", s.cycle_seq);
                s.emit(TuiEvent::CycleEnd {
                    cycle_id: cid,
                    pass_count: 1,
                    duration_ms: 500,
                });
                s.active_mut().running = false;
                s.active_mut().last_result = Some(CycleResultSummary {
                    pass_count: 1,
                    task_ledger: HashMap::new(),
                });
            }
            let _ = tx.send(());
            Ok(())
        })
    }

    fn abort(&self) {
        self.record("abort");
        {
            let mut s = self.state.lock().unwrap();
            if s.active().running {
                s.emit(TuiEvent::Error {
                    source: "operator".into(),
                    message: "Abort requested".into(),
                });
                s.active_mut().running = false;
            }
        }
        self.ping();
    }

    fn new_session(&self) {
        self.record("new_session");
        {
            let mut s = self.state.lock().unwrap();
            let session_id = gen_id("tui");
            let t = s.active_mut();
            t.messages.clear();
            t.events.clear();
            t.chat_events.clear();
            t.running = false;
            t.last_result = None;
            t.session_id = session_id;
        }
        self.ping();
    }

    fn fork(&self, name: Option<String>) -> String {
        self.record("fork");
        let id = {
            let mut s = self.state.lock().unwrap();
            let next = format!("t{}", s.threads.len() + 1);
            let (parent_id, messages, chat_events) = {
                let active = s.active();
                (
                    active.id.clone(),
                    active.messages.clone(),
                    active.chat_events.clone(),
                )
            };
            let child = Thread {
                id: next.clone(),
                parent_id: Some(parent_id),
                name: name.unwrap_or_else(|| format!("fork {next}")),
                session_id: gen_id("tui"),
                messages,
                events: chat_events.clone(),
                chat_events,
                running: false,
                last_result: None,
            };
            s.threads.push(child);
            s.active_id = next.clone();
            next
        };
        self.ping();
        id
    }

    fn set_active_thread(&self, id: String) {
        self.record("set_active_thread");
        {
            let mut s = self.state.lock().unwrap();
            if s.threads.iter().any(|t| t.id == id) {
                s.active_id = id;
            }
        }
        self.ping();
    }

    fn list_main_chats(&self) -> BoxFuture<'static, anyhow::Result<Vec<MainChatSummary>>> {
        Box::pin(async move {
            Ok(vec![
                MainChatSummary {
                    session_id: "tui-demo-1".into(),
                    name: "Auth refactor".into(),
                    turns: 4,
                    thread_count: 2,
                    updated_at: crate::ui::chat_store::iso8601_utc(now_millis()),
                },
                MainChatSummary {
                    session_id: "tui-demo-2".into(),
                    name: "Repo audit".into(),
                    turns: 2,
                    thread_count: 1,
                    updated_at: crate::ui::chat_store::iso8601_utc(now_millis() - 86_400_000),
                },
            ])
        })
    }

    fn resume_chat(&self, _main_session_id: String) -> BoxFuture<'static, anyhow::Result<()>> {
        let state = self.state.clone();
        let tx = self.tx.clone();
        Box::pin(async move {
            {
                let mut s = state.lock().unwrap();
                if s.threads.iter().any(|t| t.running) {
                    return Err(anyhow::anyhow!("cannot resume while a thread is running"));
                }
                s.active_mut().messages.push(ChatMessage {
                    role: "assistant".into(),
                    content: "(resumed chat)".into(),
                });
            }
            let _ = tx.send(());
            Ok(())
        })
    }

    fn set_async_mode(&self, on: bool) -> bool {
        self.record("set_async_mode");
        {
            let mut s = self.state.lock().unwrap();
            s.async_mode = on;
        }
        self.ping();
        on
    }

    fn inspect_context(&self) -> BoxFuture<'static, anyhow::Result<Vec<ContextItem>>> {
        Box::pin(async move {
            Ok(vec![
                ContextItem {
                    ref_: "ctx://task-1/result".into(),
                    kind: "task-result".into(),
                    bytes: 482,
                    content: "Refactored auth into 3 focused modules.".into(),
                },
                ContextItem {
                    ref_: "ctx://memory/house-rules".into(),
                    kind: "memory".into(),
                    bytes: 128,
                    content: "Always run the test suite before handoff.".into(),
                },
            ])
        })
    }

    fn shutdown(&self) -> BoxFuture<'static, anyhow::Result<()>> {
        Box::pin(async move { Ok(()) })
    }

    fn memory_status(&self) -> Option<crate::memory::MemoryStatus> {
        self.memory
            .lock()
            .unwrap()
            .as_ref()
            .and_then(|m| m.status.clone())
    }

    fn memory_search(
        &self,
        _query: String,
        _facet: Option<String>,
        k: usize,
    ) -> Vec<crate::memory::MemoryHit> {
        self.memory
            .lock()
            .unwrap()
            .as_ref()
            .map(|m| m.hits.iter().take(k).cloned().collect())
            .unwrap_or_default()
    }

    fn memory_directives(&self) -> Vec<String> {
        self.memory
            .lock()
            .unwrap()
            .as_ref()
            .map(|m| m.directives.clone())
            .unwrap_or_default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn demo_snapshot_is_populated() {
        let rt = MockRuntime::demo();
        let snap = rt.snapshot();
        assert!(!snap.events.is_empty());
        assert_eq!(snap.roster.len(), 1);
        assert!(snap.last_result.is_some());
        assert_eq!(snap.messages.len(), 2);
    }

    #[test]
    fn fork_inherits_history() {
        let rt = MockRuntime::demo();
        let before = rt.snapshot().messages.len();
        let id = rt.fork(Some("branch".into()));
        let snap = rt.snapshot();
        assert_eq!(snap.active_thread_id, id);
        assert_eq!(snap.messages.len(), before);
        assert_eq!(snap.threads.len(), 2);
    }

    #[test]
    fn async_toggle_reflected() {
        let rt = MockRuntime::empty();
        assert!(!rt.snapshot().async_mode);
        assert!(rt.set_async_mode(true));
        assert!(rt.snapshot().async_mode);
    }

    #[tokio::test]
    async fn submit_appends_turns() {
        let rt = MockRuntime::empty();
        rt.submit("hello".into()).await.unwrap();
        let snap = rt.snapshot();
        assert!(!snap.running);
        assert_eq!(snap.messages.len(), 2);
        assert!(crate::ui::events::last_assistant_message(&snap.chat_events).is_some());
        assert!(rt.recorded_calls().contains(&"submit".to_string()));
    }

    #[tokio::test]
    async fn submit_rejects_while_running() {
        let rt = MockRuntime::empty();
        rt.set_running(true);
        let err = rt.submit("hi".into()).await.unwrap_err();
        assert!(err.to_string().contains("already running"));
    }

    #[test]
    fn abort_emits_error_only_when_running() {
        let rt = MockRuntime::empty();
        // Idle abort records the call but emits nothing.
        rt.abort();
        assert!(rt.snapshot().events.is_empty());
        // While running, abort emits an operator error and clears the flag.
        rt.set_running(true);
        rt.abort();
        let snap = rt.snapshot();
        assert!(!snap.running);
        assert!(snap
            .events
            .iter()
            .any(|e| matches!(&e.event, TuiEvent::Error { source, .. } if source == "operator")));
        let calls = rt.recorded_calls();
        assert_eq!(calls.iter().filter(|c| *c == "abort").count(), 2);
    }

    #[test]
    fn new_session_clears_history_and_resets_id() {
        let rt = MockRuntime::demo();
        rt.new_session();
        let snap = rt.snapshot();
        assert!(snap.events.is_empty());
        assert!(snap.messages.is_empty());
        assert!(snap.last_result.is_none());
        assert!(!snap.running);
        // A fresh session id is (re)assigned; the clock-derived id may collide within
        // the same millisecond, so we only assert it is non-empty here.
        assert!(!snap.session_id.is_empty());
        assert!(rt.recorded_calls().contains(&"new_session".to_string()));
    }

    #[test]
    fn set_active_thread_ignores_unknown_ids() {
        let rt = MockRuntime::demo();
        rt.fork(Some("branch".into()));
        assert_eq!(rt.snapshot().active_thread_id, "t2");
        // An unknown id is a no-op; the active thread stays put.
        rt.set_active_thread("nope".into());
        assert_eq!(rt.snapshot().active_thread_id, "t2");
        // A known id switches back.
        rt.set_active_thread("t1".into());
        assert_eq!(rt.snapshot().active_thread_id, "t1");
    }

    #[tokio::test]
    async fn resume_chat_appends_and_rejects_while_running() {
        let rt = MockRuntime::empty();
        rt.resume_chat("main".into()).await.unwrap();
        assert_eq!(rt.snapshot().messages.len(), 1);
        rt.set_running(true);
        let err = rt.resume_chat("main".into()).await.unwrap_err();
        assert!(err.to_string().contains("cannot resume"));
    }

    #[tokio::test]
    async fn list_main_chats_and_inspect_context_populate() {
        let rt = MockRuntime::demo();
        let chats = rt.list_main_chats().await.unwrap();
        assert_eq!(chats.len(), 2);
        assert_eq!(chats[0].name, "Auth refactor");
        let ctx = rt.inspect_context().await.unwrap();
        assert_eq!(ctx.len(), 2);
        assert_eq!(ctx[0].kind, "task-result");
    }

    #[tokio::test]
    async fn shutdown_is_ok() {
        let rt = MockRuntime::empty();
        rt.shutdown().await.unwrap();
    }

    #[test]
    fn thread_summaries_count_running_tasks_and_attention() {
        let rt = MockRuntime::empty();
        rt.script_event(TuiEvent::TaskStart {
            task_id: "t1".into(),
            instruction: "go".into(),
            depth: 2,
            agent_id: None,
        });
        rt.script_event(TuiEvent::TaskAttention {
            task_id: "t1".into(),
            reason: "confirm".into(),
            content: "?".into(),
            question_id: Some("q".into()),
        });
        rt.script_event(TuiEvent::Error {
            source: "cycle".into(),
            message: "oops".into(),
        });
        let snap = rt.snapshot();
        let main = &snap.threads[0];
        assert_eq!(main.running_tasks, 1, "one open task");
        assert_eq!(main.attention, 2, "attention + error both count");
    }

    #[test]
    fn memory_surface_defaults_empty_and_is_scriptable() {
        use crate::memory::{MemoryHit, MemoryStatus};
        let rt = MockRuntime::empty();
        // No scripted memory → the seam is inert.
        assert!(rt.memory_status().is_none());
        assert!(rt.memory_search("q".into(), None, 5).is_empty());
        assert!(rt.memory_directives().is_empty());

        rt.set_memory_status(MemoryStatus {
            enabled: true,
            workspace: "/ws".into(),
            pack_exists: false,
            pack_path: "/ws/persona/PERSONA.md".into(),
            entry_count: 2,
            directives_count: 1,
            facet_counts: Default::default(),
        });
        rt.set_memory_directives(vec!["Always branch first".into()]);
        rt.set_memory_hits(vec![MemoryHit {
            facet: "workflow".into(),
            tier: "t0".into(),
            text: "Commit small and often".into(),
            quote: None,
            timestamp: "2020-01-01T00:00:00+00:00".into(),
            score: 1.0,
        }]);
        assert!(rt.memory_status().unwrap().enabled);
        assert_eq!(rt.memory_directives(), vec!["Always branch first"]);
        // `k` caps the scripted hits.
        assert_eq!(rt.memory_search("q".into(), None, 0).len(), 0);
        assert_eq!(rt.memory_search("q".into(), None, 5).len(), 1);
    }

    #[test]
    fn subscribe_receives_a_ping_on_mutation() {
        let rt = MockRuntime::empty();
        let mut rx = rt.subscribe();
        rt.set_async_mode(true);
        assert!(rx.try_recv().is_ok());
    }
}
