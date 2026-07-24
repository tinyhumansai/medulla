//! Feature-level tests for the Routing fleet view and header stream-health
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
use medulla_tui::ui::app::{App, Cmd, TABS};
use medulla_tui::ui::chat_store::MainChatSummary;

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
        cpu_cores: Some(8),
        memory_total_bytes: Some(32 * 1024 * 1024 * 1024),
        memory_available_bytes: Some(18 * 1024 * 1024 * 1024),
        ip_address: Some(format!("10.0.0.{}", id.trim_start_matches('w'))),
        selected,
    }
}

fn app_with_workers(stream: Option<StreamState>) -> App {
    app_with_roster(
        vec![worker("w1", true), worker("w2", false), worker("w3", false)],
        stream,
    )
}

fn app_with_roster(workers: Vec<WorkerInfo>, stream: Option<StreamState>) -> App {
    let rt: Arc<dyn Runtime> = Arc::new(FleetRuntime::new(workers, stream));
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
    tab(&mut app, "Routing");
    app.focus_routing_subpage("List Workers");
    let out = render(&mut app, 120, 40);
    assert!(out.contains("List Workers · 3"), "worker count in title");
    assert!(out.contains("@w1"));
    assert!(out.contains("CODEX"));
    assert!(out.contains("IP 10.0.0.1"));
    assert!(out.contains("CPU 8 cores"));
    assert!(out.contains("RAM 18.0 GiB available / 32.0 GiB total"));
    assert!(out.contains("a add · Enter/s select"));
}

#[test]
fn workers_r_refreshes_selected_machine_details() {
    let mut app = app_with_workers(None);
    tab(&mut app, "Routing");
    app.focus_routing_subpage("List Workers");
    let _ = app.on_event(key(KeyCode::Down));
    let cmd = app.on_event(key(KeyCode::Char('r')));
    match cmd {
        Some(Cmd::WorkerOp(op)) => {
            let debug = format!("{op:?}");
            assert!(debug.contains("RefreshDetails"));
            assert!(debug.contains("w2"));
        }
        other => panic!("expected details refresh, got {other:?}"),
    }
}

#[test]
fn routing_nav_exposes_all_four_subpages() {
    let mut app = app_with_workers(None);
    tab(&mut app, "Routing");
    let out = render(&mut app, 120, 40);
    for page in ["List Workers", "Add Worker", "Manage Keys", "Strategies"] {
        assert!(out.contains(page), "missing {page}: {out}");
    }
}

#[test]
fn routing_menu_enters_leaves_and_jumps_between_content_panes() {
    let mut app = app_with_workers(None);
    tab(&mut app, "Routing");
    assert!(!app.routing_focused());

    assert!(app.on_event(key(KeyCode::Down)).is_none());
    assert_eq!(app.routing_subpage(), "Add Worker");
    assert!(app.on_event(key(KeyCode::Enter)).is_none());
    assert!(app.routing_focused());
    assert!(app.on_event(key(KeyCode::Esc)).is_none());
    assert!(!app.routing_focused());

    assert!(app.on_event(key(KeyCode::Char('4'))).is_none());
    assert_eq!(app.routing_subpage(), "Strategies");
    assert!(app.routing_focused());
}

#[test]
fn routing_pages_leave_unbound_keys_for_global_handling() {
    let mut app = app_with_workers(None);
    for page in ["Add Worker", "Manage Keys", "Strategies"] {
        app.focus_routing_subpage(page);
        assert!(app.on_event(key(KeyCode::Char('z'))).is_none());
    }
}

#[test]
fn add_worker_page_renders_guidance_and_opens_the_prompt() {
    let mut app = app_with_workers(None);
    app.focus_routing_subpage("Add Worker");

    let out = render(&mut app, 120, 40);
    assert!(out.contains("Connect a tiny.place worker"));
    assert!(out.contains("@build-box Primary build worker"));

    assert!(app.on_event(key(KeyCode::Enter)).is_none());
    let (title, draft) = app.prompt_state().expect("add prompt");
    assert!(title.starts_with("Add worker"));
    assert!(draft.is_empty());
}

#[test]
fn worker_list_add_shortcut_opens_the_shared_prompt() {
    let mut app = app_with_workers(None);
    app.focus_routing_subpage("List Workers");

    assert!(app.on_event(key(KeyCode::Char('a'))).is_none());
    assert_eq!(app.routing_subpage(), "Add Worker");
    assert!(app.prompt_state().is_some());
}

#[test]
fn empty_worker_list_explains_the_state_and_roster_actions_are_noops() {
    let mut app = app_with_roster(Vec::new(), None);
    app.focus_routing_subpage("List Workers");

    let out = render(&mut app, 120, 40);
    assert!(out.contains("No workers registered"));
    for code in [
        KeyCode::Enter,
        KeyCode::Char('d'),
        KeyCode::Char('e'),
        KeyCode::Char('r'),
    ] {
        assert!(app.on_event(key(code)).is_none());
    }
}

#[test]
fn worker_list_formats_missing_details_and_megabytes() {
    let mut missing = worker("w1", true);
    missing.ip_address = None;
    missing.cpu_cores = None;
    missing.memory_available_bytes = None;
    missing.memory_total_bytes = None;
    missing.handle = None;
    missing.label = None;
    missing.harness = None;

    let mut small = worker("w2", false);
    small.memory_available_bytes = Some(512 * 1024 * 1024);
    small.memory_total_bytes = Some(768 * 1024 * 1024);

    let mut app = app_with_roster(vec![missing, small], None);
    app.focus_routing_subpage("List Workers");
    let out = render(&mut app, 120, 40);

    assert!(out.contains("details not captured"));
    assert!(out.contains("512 MiB available / 768 MiB total"));
}

#[test]
fn worker_list_paginates_by_two_line_rows() {
    let workers = (1..=12)
        .map(|index| worker(&format!("w{index}"), index == 1))
        .collect();
    let mut app = app_with_roster(workers, None);
    app.focus_routing_subpage("List Workers");
    for _ in 1..12 {
        let _ = app.on_event(key(KeyCode::Down));
    }

    let out = render(&mut app, 120, 24);
    assert!(out.contains("w12"), "selected worker should remain visible");
    assert!(
        out.contains("refresh details"),
        "action footer should remain visible"
    );
}

#[test]
fn strategies_choose_and_apply_a_capacity_policy() {
    let mut app = app_with_workers(None);
    app.focus_routing_subpage("Strategies");
    let _ = app.on_event(key(KeyCode::Down));
    let _ = app.on_event(key(KeyCode::Down));
    let _ = app.on_event(key(KeyCode::Up));
    let _ = app.on_event(key(KeyCode::Down));
    let out = render(&mut app, 120, 40);
    assert!(out.contains("CPU First"));
    assert!(out.contains("most logical CPU cores"));
    let cmd = app.on_event(key(KeyCode::Enter));
    match cmd {
        Some(Cmd::WorkerOp(op)) => assert!(format!("{op:?}").contains("CpuFirst")),
        other => panic!("expected strategy command, got {other:?}"),
    }
}

#[test]
fn manage_keys_names_subscriptions_and_api_sources_without_values() {
    let mut app = app_with_workers(None);
    app.focus_routing_subpage("Manage Keys");
    let out = render(&mut app, 120, 40);
    assert!(out.contains("Provider subscriptions"));
    assert!(out.contains("Claude Code"));
    assert!(out.contains("Codex"));
    assert!(out.contains("Anthropic"));
    assert!(out.contains("OpenRouter"));
    assert!(out.contains("Press r to refresh"));
    assert!(out.contains("Secret values are never rendered"));
    assert!(app.on_event(key(KeyCode::Char('r'))).is_none());
    assert_eq!(app.status(), "Credential status refreshed");
}

#[test]
fn workers_up_down_moves_selection() {
    let mut app = app_with_workers(None);
    tab(&mut app, "Routing");
    app.focus_routing_subpage("List Workers");
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
    tab(&mut app, "Routing");
    app.focus_routing_subpage("List Workers");
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
    tab(&mut app, "Routing");
    app.focus_routing_subpage("List Workers");
    let cmd = app.on_event(key(KeyCode::Char('s')));
    assert!(matches!(cmd, Some(Cmd::WorkerOp(_))));
    let cmd = app.on_event(key(KeyCode::Char('x')));
    assert!(matches!(cmd, Some(Cmd::WorkerOp(_))));
}

#[test]
fn workers_e_opens_edit_label_prompt_prefilled() {
    let mut app = app_with_workers(None);
    tab(&mut app, "Routing");
    app.focus_routing_subpage("List Workers");
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
