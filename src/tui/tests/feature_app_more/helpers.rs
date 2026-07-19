//! Shared setup for the `feature_app_more` test binary: app constructors, a
//! worker-exposing `FleetRuntime`, synthetic crossterm event builders, and a
//! `TestBackend` render helper. Re-exports the crossterm/ratatui/medulla types
//! the grouped test modules need so they can `use crate::helpers::*;`.

pub use std::sync::Arc;

pub use crossterm::event::{
    Event, KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind,
};
pub use ratatui::backend::TestBackend;
pub use ratatui::Terminal;

pub use medulla::config::{LoadedConfig, TinyplaceConfig};
pub use medulla::runtime::mock::MockRuntime;
pub use medulla::runtime::Runtime;
pub use medulla_tui::ui::app::{App, Cmd, TABS};
pub use medulla_tui::ui::events::{TaskDigest, TuiEvent, Usage};

pub fn loaded() -> LoadedConfig {
    let mut l = LoadedConfig::defaults("medulla.tui.json".into());
    l.config.tinyplace = Some(TinyplaceConfig::default());
    l
}

pub fn demo_app() -> (App, Arc<MockRuntime>) {
    let rt = Arc::new(MockRuntime::demo());
    let app = App::new(rt.clone(), loaded());
    (app, rt)
}

pub fn empty_app() -> (App, Arc<MockRuntime>) {
    let rt = Arc::new(MockRuntime::empty());
    let app = App::new(rt.clone(), loaded());
    (app, rt)
}

/// A runtime that exposes a worker registry and a live stream state on top of a
/// `MockRuntime`, so the Workers tab and the header stream-health indicator have
/// something to render. Everything else delegates to the inner mock.
pub struct FleetRuntime {
    pub inner: Arc<MockRuntime>,
    pub workers: Vec<medulla::runtime::WorkerInfo>,
}

impl medulla::runtime::Runtime for FleetRuntime {
    fn snapshot(&self) -> medulla::runtime::RuntimeSnapshot {
        self.inner.snapshot()
    }
    fn subscribe(&self) -> tokio::sync::broadcast::Receiver<()> {
        self.inner.subscribe()
    }
    fn submit(&self, input: String) -> futures::future::BoxFuture<'static, anyhow::Result<()>> {
        self.inner.submit(input)
    }
    fn abort(&self) {
        self.inner.abort()
    }
    fn new_session(&self) {
        self.inner.new_session()
    }
    fn fork(&self, name: Option<String>) -> String {
        self.inner.fork(name)
    }
    fn set_active_thread(&self, id: String) {
        self.inner.set_active_thread(id)
    }
    fn list_main_chats(
        &self,
    ) -> futures::future::BoxFuture<
        'static,
        anyhow::Result<Vec<medulla_tui::ui::chat_store::MainChatSummary>>,
    > {
        self.inner.list_main_chats()
    }
    fn resume_chat(&self, id: String) -> futures::future::BoxFuture<'static, anyhow::Result<()>> {
        self.inner.resume_chat(id)
    }
    fn set_async_mode(&self, on: bool) -> bool {
        self.inner.set_async_mode(on)
    }
    fn inspect_context(
        &self,
    ) -> futures::future::BoxFuture<'static, anyhow::Result<Vec<medulla::runtime::ContextItem>>>
    {
        self.inner.inspect_context()
    }
    fn shutdown(&self) -> futures::future::BoxFuture<'static, anyhow::Result<()>> {
        self.inner.shutdown()
    }
    fn workers(&self) -> Vec<medulla::runtime::WorkerInfo> {
        self.workers.clone()
    }
    fn stream_state(&self) -> Option<medulla::runtime::StreamState> {
        Some(medulla::runtime::StreamState::Live)
    }
}

pub fn fleet_app() -> App {
    let inner = Arc::new(MockRuntime::demo());
    inner.set_running(true); // header shows stream health only while running
    let rt = Arc::new(FleetRuntime {
        inner,
        workers: vec![medulla::runtime::WorkerInfo {
            id: "w_1".into(),
            address: "@dev".into(),
            handle: Some("@dev".into()),
            label: Some("primary".into()),
            harness: Some("claude".into()),
            peer_id: None,
            selected: true,
        }],
    });
    App::new(rt, loaded())
}

pub fn key(code: KeyCode) -> Event {
    Event::Key(KeyEvent::new(code, KeyModifiers::NONE))
}

pub fn ctrl(code: KeyCode) -> Event {
    Event::Key(KeyEvent::new(code, KeyModifiers::CONTROL))
}

pub fn mouse(kind: MouseEventKind, column: u16, row: u16) -> Event {
    Event::Mouse(MouseEvent {
        kind,
        column,
        row,
        modifiers: KeyModifiers::NONE,
    })
}

pub fn type_str(app: &mut App, s: &str) {
    for ch in s.chars() {
        let _ = app.on_event(key(KeyCode::Char(ch)));
    }
}

pub fn submit_line(app: &mut App, s: &str) -> Option<Cmd> {
    app.tab_index = 1;
    type_str(app, s);
    app.on_event(key(KeyCode::Enter))
}

pub fn render(app: &mut App, w: u16, h: u16) -> String {
    let mut terminal = Terminal::new(TestBackend::new(w, h)).unwrap();
    terminal.draw(|f| app.draw(f)).unwrap();
    let buf = terminal.backend().buffer().clone();
    buf.content().iter().map(|c| c.symbol()).collect()
}

pub fn tab(app: &mut App, name: &str) {
    app.tab_index = TABS.iter().position(|t| *t == name).unwrap();
}

/// Script a running, cycle-scoped delegated task with a pending question onto the
/// demo runtime and drive the Agents cursor onto its Sub row.
pub fn app_with_selected_task() -> (App, Arc<MockRuntime>) {
    let (mut app, rt) = demo_app();
    rt.script_event(TuiEvent::TaskStart {
        task_id: "cyc-9/t:q1".into(),
        instruction: "needs a decision".into(),
        depth: 2,
        agent_id: Some("dev-1".into()),
    });
    rt.script_event(TuiEvent::TaskAttention {
        task_id: "cyc-9/t:q1".into(),
        reason: "confirm".into(),
        content: "proceed?".into(),
        question_id: Some("qid-1".into()),
    });
    app.refresh_snapshot();
    tab(&mut app, "Agents");
    // Walk the cursor down until a task row is selected (running tasks sort first).
    for _ in 0..12 {
        if app.selected_task_id().is_some() {
            break;
        }
        let _ = app.on_event(key(KeyCode::Down));
    }
    (app, rt)
}
