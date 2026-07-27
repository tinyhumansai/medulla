//! Shared setup for the `feature_workers` test binary: a worker-exposing
//! `FleetRuntime` on top of `MockRuntime`, app constructors seeded with a roster,
//! synthetic crossterm event builders, and a `TestBackend` render helper.
//! Re-exports the crossterm/ratatui/medulla types the grouped test modules need
//! so they can `use crate::helpers::*;`.

pub use std::sync::{Arc, Mutex};

pub use crossterm::event::{Event, KeyCode, KeyEvent, KeyModifiers};
pub use futures::future::BoxFuture;
pub use ratatui::backend::TestBackend;
pub use ratatui::Terminal;
pub use tokio::sync::broadcast;

pub use medulla::config::LoadedConfig;
pub use medulla::runtime::mock::MockRuntime;
pub use medulla::runtime::{
    ContextItem, Runtime, RuntimeSnapshot, StreamState, WorkerInfo, WorkerOp,
};
pub use medulla::tinyplace::{
    BudgetSource, BudgetWindow, HarnessBudget, HarnessProvider, HarnessReadiness,
};
pub use medulla_tui::ui::app::{App, Cmd, TABS};
pub use medulla_tui::ui::chat_store::MainChatSummary;

/// A `Runtime` with a populated worker registry and a fixed stream state, built on
/// top of a `MockRuntime` for everything else.
pub struct FleetRuntime {
    inner: MockRuntime,
    workers: Vec<WorkerInfo>,
    stream: Option<StreamState>,
    ops: Arc<Mutex<Vec<String>>>,
}

impl FleetRuntime {
    pub fn new(workers: Vec<WorkerInfo>, stream: Option<StreamState>) -> Self {
        FleetRuntime {
            inner: MockRuntime::demo(),
            workers,
            stream,
            ops: Arc::new(Mutex::new(Vec::new())),
        }
    }
}

impl Runtime for FleetRuntime {
    fn snapshot(&self) -> RuntimeSnapshot {
        self.inner.snapshot()
    }
    fn subscribe(&self) -> broadcast::Receiver<()> {
        self.inner.subscribe()
    }
    fn submit(&self, input: String) -> BoxFuture<'static, anyhow::Result<()>> {
        self.inner.submit(input)
    }
    fn abort(&self) {
        self.inner.abort()
    }
    fn new_session(&self) {
        self.inner.new_session()
    }
    fn set_active_thread(&self, id: String) {
        self.inner.set_active_thread(id)
    }
    fn list_main_chats(&self) -> BoxFuture<'static, anyhow::Result<Vec<MainChatSummary>>> {
        self.inner.list_main_chats()
    }
    fn resume_chat(&self, id: String) -> BoxFuture<'static, anyhow::Result<()>> {
        self.inner.resume_chat(id)
    }
    fn inspect_context(&self) -> BoxFuture<'static, anyhow::Result<Vec<ContextItem>>> {
        self.inner.inspect_context()
    }
    fn shutdown(&self) -> BoxFuture<'static, anyhow::Result<()>> {
        self.inner.shutdown()
    }
    fn workers(&self) -> Vec<WorkerInfo> {
        self.workers.clone()
    }
    fn worker_op(&self, op: WorkerOp) -> BoxFuture<'static, anyhow::Result<()>> {
        self.ops.lock().unwrap().push(format!("{op:?}"));
        Box::pin(async { Ok(()) })
    }
    fn stream_state(&self) -> Option<StreamState> {
        self.stream
    }
}

/// A representative worker row: full capacity details, no budgets/readiness.
pub fn worker(id: &str, selected: bool) -> WorkerInfo {
    WorkerInfo {
        id: id.into(),
        address: format!("{id}.example:9000"),
        handle: Some(format!("@{id}")),
        label: Some(format!("{id} label")),
        harness: Some("codex".into()),
        workspace: None,
        peer_id: Some(format!("peer-{id}")),
        cpu_cores: Some(8),
        memory_total_bytes: Some(32 * 1024 * 1024 * 1024),
        memory_available_bytes: Some(18 * 1024 * 1024 * 1024),
        ip_address: Some(format!("10.0.0.{}", id.trim_start_matches('w'))),
        selected,
        budgets: Vec::new(),
        readiness: Vec::new(),
    }
}

/// An `App` over a three-worker roster (`w1` selected).
pub fn app_with_workers(stream: Option<StreamState>) -> App {
    app_with_roster(
        vec![worker("w1", true), worker("w2", false), worker("w3", false)],
        stream,
    )
}

/// An `App` over an explicit roster, useful for empty/edge rosters.
pub fn app_with_roster(workers: Vec<WorkerInfo>, stream: Option<StreamState>) -> App {
    let rt: Arc<dyn Runtime> = Arc::new(FleetRuntime::new(workers, stream));
    App::new(rt, LoadedConfig::defaults("medulla.tui.json".into()))
}

/// A plain (no-modifier) key event.
pub fn key(code: KeyCode) -> Event {
    Event::Key(KeyEvent::new(code, KeyModifiers::NONE))
}

/// An `Alt`-modified key: lane and rail navigation on the Agents tab, since the
/// bare arrows belong to the composer.
pub fn alt_key(code: KeyCode) -> Event {
    Event::Key(KeyEvent::new(code, KeyModifiers::ALT))
}

/// Jump the app to the named top-level tab.
pub fn tab(app: &mut App, name: &str) {
    app.tab_index = TABS.iter().position(|t| *t == name).unwrap();
}

/// Render the app to a `w`×`h` `TestBackend` and return the flattened glyphs.
pub fn render(app: &mut App, w: u16, h: u16) -> String {
    let mut terminal = Terminal::new(TestBackend::new(w, h)).unwrap();
    terminal.draw(|f| app.draw(f)).unwrap();
    let buf = terminal.backend().buffer().clone();
    buf.content().iter().map(|c| c.symbol()).collect()
}
