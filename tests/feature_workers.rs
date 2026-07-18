//! Feature-level tests for the Workers fleet tab and the header stream-health
//! indicator. These need a `Runtime` that actually surfaces workers and a stream
//! state, so a thin `FleetRuntime` wraps `MockRuntime`, delegating everything but
//! `workers()` / `worker_op()` / `stream_state()`.

use std::sync::{Arc, Mutex};

use crossterm::event::{Event, KeyCode, KeyEvent, KeyModifiers};
use futures::future::BoxFuture;
use ratatui::backend::TestBackend;
use ratatui::Terminal;
use tokio::sync::broadcast;

use medulla::config::LoadedConfig;
use medulla::runtime::mock::MockRuntime;
use medulla::runtime::{ContextItem, Runtime, RuntimeSnapshot, StreamState, WorkerInfo, WorkerOp};
use medulla::ui::app::{App, Cmd, TABS};
use medulla::ui::chat_store::MainChatSummary;

/// A `Runtime` with a populated worker registry and a fixed stream state, built on
/// top of a `MockRuntime` for everything else.
struct FleetRuntime {
    inner: MockRuntime,
    workers: Vec<WorkerInfo>,
    stream: Option<StreamState>,
    ops: Arc<Mutex<Vec<String>>>,
}

impl FleetRuntime {
    fn new(workers: Vec<WorkerInfo>, stream: Option<StreamState>) -> Self {
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
    fn fork(&self, name: Option<String>) -> String {
        self.inner.fork(name)
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
    fn set_async_mode(&self, on: bool) -> bool {
        self.inner.set_async_mode(on)
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

fn worker(id: &str, selected: bool) -> WorkerInfo {
    WorkerInfo {
        id: id.into(),
        address: format!("{id}.example:9000"),
        handle: Some(format!("@{id}")),
        label: Some(format!("{id} label")),
        harness: Some("codex".into()),
        peer_id: Some(format!("peer-{id}")),
        selected,
    }
}

fn app_with_workers(stream: Option<StreamState>) -> App {
    let rt: Arc<dyn Runtime> = Arc::new(FleetRuntime::new(
        vec![worker("w1", true), worker("w2", false), worker("w3", false)],
        stream,
    ));
    App::new(rt, LoadedConfig::defaults("medulla.tui.json".into()))
}

fn key(code: KeyCode) -> Event {
    Event::Key(KeyEvent::new(code, KeyModifiers::NONE))
}

fn tab(app: &mut App, name: &str) {
    app.tab_index = TABS.iter().position(|t| *t == name).unwrap();
}

fn render(app: &mut App, w: u16, h: u16) -> String {
    let mut terminal = Terminal::new(TestBackend::new(w, h)).unwrap();
    terminal.draw(|f| app.draw(f)).unwrap();
    let buf = terminal.backend().buffer().clone();
    buf.content().iter().map(|c| c.symbol()).collect()
}

#[test]
fn workers_tab_lists_registered_peers() {
    let mut app = app_with_workers(None);
    tab(&mut app, "Workers");
    let out = render(&mut app, 120, 40);
    assert!(out.contains("Workers · 3"), "worker count in title");
    assert!(out.contains("@w1"));
    assert!(out.contains("CODEX"));
    assert!(out.contains("a add · Enter/s select"));
}

#[test]
fn workers_up_down_moves_selection() {
    let mut app = app_with_workers(None);
    tab(&mut app, "Workers");
    assert_eq!(app.worker_index(), 0);
    let _ = app.on_event(key(KeyCode::Down));
    assert_eq!(app.worker_index(), 1);
    let _ = app.on_event(key(KeyCode::Down));
    assert_eq!(app.worker_index(), 2);
    // Clamp at the last worker.
    let _ = app.on_event(key(KeyCode::Down));
    assert_eq!(app.worker_index(), 2);
    let _ = app.on_event(key(KeyCode::Up));
    assert_eq!(app.worker_index(), 1);
}

#[test]
fn workers_enter_selects_and_d_removes() {
    let mut app = app_with_workers(None);
    tab(&mut app, "Workers");
    let _ = app.on_event(key(KeyCode::Down)); // select w2
    let cmd = app.on_event(key(KeyCode::Enter));
    match cmd {
        Some(Cmd::WorkerOp(op)) => assert!(format!("{op:?}").contains("Select")),
        other => panic!("expected Select, got {other:?}"),
    }
    let cmd = app.on_event(key(KeyCode::Char('d')));
    match cmd {
        Some(Cmd::WorkerOp(op)) => assert!(format!("{op:?}").contains("Remove")),
        other => panic!("expected Remove, got {other:?}"),
    }
}

#[test]
fn workers_s_and_x_are_select_and_remove_aliases() {
    let mut app = app_with_workers(None);
    tab(&mut app, "Workers");
    let cmd = app.on_event(key(KeyCode::Char('s')));
    assert!(matches!(cmd, Some(Cmd::WorkerOp(_))));
    let cmd = app.on_event(key(KeyCode::Char('x')));
    assert!(matches!(cmd, Some(Cmd::WorkerOp(_))));
}

#[test]
fn workers_e_opens_edit_label_prompt_prefilled() {
    let mut app = app_with_workers(None);
    tab(&mut app, "Workers");
    let _ = app.on_event(key(KeyCode::Char('e')));
    let (title, draft) = app.prompt_state().expect("edit prompt open");
    assert!(title.starts_with("Edit label"));
    // Prefilled with the current label.
    assert_eq!(draft, "w1 label");
    // Editing and submitting produces an Update WorkerOp.
    let _ = app.on_event(key(KeyCode::Backspace));
    let cmd = app.on_event(key(KeyCode::Enter));
    match cmd {
        Some(Cmd::WorkerOp(op)) => {
            let dbg = format!("{op:?}");
            assert!(dbg.contains("Update"));
            assert!(dbg.contains("label"));
        }
        other => panic!("expected Update, got {other:?}"),
    }
}

#[test]
fn idle_header_omits_stream_health() {
    // Stream health only shows while a cycle runs; the demo mock is idle.
    let mut app = app_with_workers(Some(StreamState::Resyncing));
    let out = render(&mut app, 120, 40);
    assert!(
        !out.contains("resyncing"),
        "idle header omits stream health"
    );
}

#[test]
fn header_shows_stream_glyph_for_running_cycle() {
    // A dedicated runtime whose snapshot reports running=true, with a Stalled stream.
    struct RunningFleet(MockRuntime);
    impl RunningFleet {
        fn new() -> Self {
            let m = MockRuntime::empty();
            m.set_running(true);
            RunningFleet(m)
        }
    }
    impl Runtime for RunningFleet {
        fn snapshot(&self) -> RuntimeSnapshot {
            self.0.snapshot()
        }
        fn subscribe(&self) -> broadcast::Receiver<()> {
            self.0.subscribe()
        }
        fn submit(&self, input: String) -> BoxFuture<'static, anyhow::Result<()>> {
            self.0.submit(input)
        }
        fn abort(&self) {
            self.0.abort()
        }
        fn new_session(&self) {
            self.0.new_session()
        }
        fn fork(&self, name: Option<String>) -> String {
            self.0.fork(name)
        }
        fn set_active_thread(&self, id: String) {
            self.0.set_active_thread(id)
        }
        fn list_main_chats(&self) -> BoxFuture<'static, anyhow::Result<Vec<MainChatSummary>>> {
            self.0.list_main_chats()
        }
        fn resume_chat(&self, id: String) -> BoxFuture<'static, anyhow::Result<()>> {
            self.0.resume_chat(id)
        }
        fn set_async_mode(&self, on: bool) -> bool {
            self.0.set_async_mode(on)
        }
        fn inspect_context(&self) -> BoxFuture<'static, anyhow::Result<Vec<ContextItem>>> {
            self.0.inspect_context()
        }
        fn shutdown(&self) -> BoxFuture<'static, anyhow::Result<()>> {
            self.0.shutdown()
        }
        fn stream_state(&self) -> Option<StreamState> {
            Some(StreamState::Stalled)
        }
    }

    let rt: Arc<dyn Runtime> = Arc::new(RunningFleet::new());
    let mut app = App::new(rt, LoadedConfig::defaults("medulla.tui.json".into()));
    let out = render(&mut app, 120, 40);
    assert!(
        out.contains("stalled"),
        "running header shows stream health"
    );
}
