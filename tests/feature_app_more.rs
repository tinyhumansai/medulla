//! Feature-level tests (batch 2): more whole-flow `App` coverage — slash-command
//! variants, Agents steering (X/A), the inline prompt overlay, Context navigation,
//! mouse routing, and the remaining composer edits. Driven via synthetic crossterm
//! events against a `MockRuntime`, asserting on observable state and rendered
//! `TestBackend` buffers.

use std::sync::Arc;

use crossterm::event::{
    Event, KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind,
};
use ratatui::backend::TestBackend;
use ratatui::Terminal;

use medulla::config::{LoadedConfig, TinyplaceConfig};
use medulla::runtime::mock::MockRuntime;
use medulla::runtime::Runtime;
use medulla::ui::app::{App, Cmd, TABS};
use medulla::ui::events::{TaskDigest, TuiEvent, Usage};

fn loaded() -> LoadedConfig {
    let mut l = LoadedConfig::defaults("medulla.tui.json".into());
    l.config.tinyplace = Some(TinyplaceConfig::default());
    l
}

fn demo_app() -> (App, Arc<MockRuntime>) {
    let rt = Arc::new(MockRuntime::demo());
    let app = App::new(rt.clone(), loaded());
    (app, rt)
}

fn empty_app() -> (App, Arc<MockRuntime>) {
    let rt = Arc::new(MockRuntime::empty());
    let app = App::new(rt.clone(), loaded());
    (app, rt)
}

/// A runtime that exposes a worker registry and a live stream state on top of a
/// `MockRuntime`, so the Workers tab and the header stream-health indicator have
/// something to render. Everything else delegates to the inner mock.
struct FleetRuntime {
    inner: Arc<MockRuntime>,
    workers: Vec<medulla::runtime::WorkerInfo>,
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
        anyhow::Result<Vec<medulla::ui::chat_store::MainChatSummary>>,
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

fn fleet_app() -> App {
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

fn key(code: KeyCode) -> Event {
    Event::Key(KeyEvent::new(code, KeyModifiers::NONE))
}

fn ctrl(code: KeyCode) -> Event {
    Event::Key(KeyEvent::new(code, KeyModifiers::CONTROL))
}

fn mouse(kind: MouseEventKind, column: u16, row: u16) -> Event {
    Event::Mouse(MouseEvent {
        kind,
        column,
        row,
        modifiers: KeyModifiers::NONE,
    })
}

fn type_str(app: &mut App, s: &str) {
    for ch in s.chars() {
        let _ = app.on_event(key(KeyCode::Char(ch)));
    }
}

fn submit_line(app: &mut App, s: &str) -> Option<Cmd> {
    app.tab_index = 1;
    type_str(app, s);
    app.on_event(key(KeyCode::Enter))
}

fn render(app: &mut App, w: u16, h: u16) -> String {
    let mut terminal = Terminal::new(TestBackend::new(w, h)).unwrap();
    terminal.draw(|f| app.draw(f)).unwrap();
    let buf = terminal.backend().buffer().clone();
    buf.content().iter().map(|c| c.symbol()).collect()
}

fn tab(app: &mut App, name: &str) {
    app.tab_index = TABS.iter().position(|t| *t == name).unwrap();
}

// --- slash-command variants -------------------------------------------------

#[test]
fn slash_resume_emits_list_chats_cmd() {
    let (mut app, _rt) = empty_app();
    let cmd = submit_line(&mut app, "/resume");
    assert!(matches!(cmd, Some(Cmd::ListChats)));
}

#[test]
fn slash_new_and_abort_and_clear_set_status() {
    let (mut app, rt) = demo_app();
    let _ = submit_line(&mut app, "/new");
    assert!(app.status().contains("fresh"), "status: {}", app.status());
    let _ = submit_line(&mut app, "/abort");
    assert!(app.status().contains("Abort"), "status: {}", app.status());
    let _ = submit_line(&mut app, "/clear");
    assert!(
        app.status().contains("View reset"),
        "status: {}",
        app.status()
    );
    let calls = rt.recorded_calls();
    assert!(calls.iter().any(|c| c == "new_session"));
    assert!(calls.iter().any(|c| c == "abort"));
}

#[test]
fn slash_fork_with_name_focuses_chat_and_names_thread() {
    let (mut app, rt) = demo_app();
    // Start from a non-Chat tab to prove the fork focuses Chat.
    tab(&mut app, "Agents");
    let _ = submit_line(&mut app, "/fork My Branch");
    assert_eq!(app.tab(), "Chat");
    assert!(rt.recorded_calls().iter().any(|c| c == "fork"));
    // The forked thread carried the (case-preserved) name.
    let out = render(&mut app, 120, 40);
    assert!(out.contains("My Branch"), "thread name should render");
}

#[test]
fn slash_copy_last_and_bad_arg() {
    let (mut app, rt) = empty_app();
    rt.script_event(TuiEvent::Assistant {
        body: "the reply".into(),
    });
    app.refresh_snapshot();
    let sink = app.capture_clipboard();
    let _ = submit_line(&mut app, "/copy last");
    assert_eq!(sink.lock().unwrap().len(), 1);
    assert!(
        app.status().contains("last reply"),
        "status: {}",
        app.status()
    );
    // An unknown scope argument is rejected with usage.
    let _ = submit_line(&mut app, "/copy sideways");
    assert!(
        app.status().contains("Usage: /copy"),
        "status: {}",
        app.status()
    );
}

#[test]
fn slash_copy_last_without_reply_reports_nothing() {
    let (mut app, _rt) = empty_app();
    let _ = submit_line(&mut app, "/copy last");
    assert!(
        app.status().contains("No assistant reply"),
        "status: {}",
        app.status()
    );
}

#[test]
fn slash_async_explicit_on_off_and_bad_arg() {
    let (mut app, _rt) = empty_app();
    let _ = submit_line(&mut app, "/async on");
    assert!(app.snapshot.async_mode);
    let _ = submit_line(&mut app, "/async off");
    assert!(!app.snapshot.async_mode);
    let _ = submit_line(&mut app, "/async maybe");
    assert!(
        app.status().contains("Usage: /async"),
        "status: {}",
        app.status()
    );
}

#[test]
fn empty_submission_is_ignored() {
    let (mut app, _rt) = empty_app();
    let cmd = submit_line(&mut app, "   ");
    assert!(cmd.is_none());
}

// --- tab -> context inspect cmd ---------------------------------------------

#[test]
fn tab_into_context_requests_inspect() {
    let (mut app, _rt) = demo_app();
    // Context is index 5; Tab from Config(6)? Start at Context-1 and Tab forward.
    app.tab_index = TABS.iter().position(|t| *t == "Trace").unwrap();
    let cmd = app.on_event(key(KeyCode::Tab)); // Trace -> Context
    assert_eq!(app.tab(), "Context");
    assert!(matches!(cmd, Some(Cmd::InspectContext)));
    // BackTab back into Context from Config too.
    app.tab_index = TABS.iter().position(|t| *t == "Config").unwrap();
    let cmd = app.on_event(Event::Key(KeyEvent::new(
        KeyCode::BackTab,
        KeyModifiers::SHIFT,
    )));
    assert_eq!(app.tab(), "Context");
    assert!(matches!(cmd, Some(Cmd::InspectContext)));
}

// --- Agents steering: X cancel / A answer -----------------------------------

/// Script a running, cycle-scoped delegated task with a pending question onto the
/// demo runtime and drive the Agents cursor onto its Sub row.
fn app_with_selected_task() -> (App, Arc<MockRuntime>) {
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

// --- Agents lane rendering: Sub rows, the More overflow, task transcript -----

#[test]
fn agents_renders_subtask_rows_more_overflow_and_task_transcript() {
    let (mut app, rt) = demo_app();
    // Delegate ten tasks to the rostered dev-1 agent so its lane overflows the
    // 8-subtask cap (→ a `More` row) and shows individual `Sub` rows.
    for n in 0..10 {
        rt.script_event(TuiEvent::TaskStart {
            task_id: format!("cyc-1/t:job{n}"),
            instruction: format!("job number {n}"),
            depth: 2,
            agent_id: Some("dev-1".into()),
        });
    }
    // One completes with usage, which lights the lane's context-token bar.
    rt.script_event(TuiEvent::TaskComplete {
        digest: TaskDigest {
            task_id: "cyc-1/t:job0".into(),
            status: "done".into(),
            digest: "finished job 0".into(),
            result_ref: None,
            usage: Some(Usage {
                input_tokens: 5000,
                output_tokens: 300,
            }),
            depth: 2,
        },
    });
    app.refresh_snapshot();
    tab(&mut app, "Agents");
    let out = render(&mut app, 120, 40);
    assert!(out.contains("more"), "the +N more overflow row renders");
    assert!(out.contains("job"), "sub-task rows render");

    // Drive the cursor onto a task row and re-render to exercise the task
    // transcript pane (and its context bar) rather than the lane transcript.
    for _ in 0..20 {
        if app.selected_task_id().is_some() {
            break;
        }
        let _ = app.on_event(key(KeyCode::Down));
    }
    assert!(app.selected_task_id().is_some(), "landed on a Sub row");
    let out = render(&mut app, 120, 40);
    assert!(out.contains("turns"), "task transcript header renders");
}

#[test]
fn agents_x_on_a_lane_row_prompts_to_select_a_task() {
    let (mut app, _rt) = demo_app();
    tab(&mut app, "Agents");
    // Default cursor sits on a tier lane, not a task row.
    assert!(app.selected_task_id().is_none());
    let _ = app.on_event(key(KeyCode::Char('X')));
    assert!(
        app.status().contains("Select a running task"),
        "status: {}",
        app.status()
    );
}

#[test]
fn agents_x_cancels_selected_cycle_task() {
    let (mut app, _rt) = app_with_selected_task();
    assert_eq!(app.selected_task_id().as_deref(), Some("cyc-9/t:q1"));
    let _ = app.on_event(key(KeyCode::Char('X')));
    // The bare task id (after the cycle prefix) appears in the confirmation.
    assert!(
        app.status().contains("Cancel requested") && app.status().contains("q1"),
        "status: {}",
        app.status()
    );
}

#[test]
fn agents_a_opens_the_answer_prompt() {
    let (mut app, _rt) = app_with_selected_task();
    let _ = app.on_event(key(KeyCode::Char('A')));
    let (title, draft) = app.prompt_state().expect("answer prompt should open");
    assert!(title.starts_with("Answer"), "title: {title}");
    assert!(draft.is_empty());
    // Rendering the overlay shows the magenta prompt caret.
    let out = render(&mut app, 120, 40);
    assert!(out.contains("Answer"), "prompt title should render");
}

#[test]
fn agents_a_on_task_without_question_reports_none() {
    // The demo's task-1 is complete with no pending question.
    let (mut app, _rt) = demo_app();
    tab(&mut app, "Agents");
    for _ in 0..12 {
        if app.selected_task_id().is_some() {
            break;
        }
        let _ = app.on_event(key(KeyCode::Down));
    }
    // Only proceed if we actually landed on the (question-less) task row.
    if app.selected_task_id().is_some() {
        let _ = app.on_event(key(KeyCode::Char('A')));
        assert!(
            app.status().contains("no pending question") || app.prompt_state().is_none(),
            "status: {}",
            app.status()
        );
    }
}

// --- inline prompt overlay editing ------------------------------------------

#[test]
fn prompt_answer_typing_editing_and_send() {
    let (mut app, _rt) = app_with_selected_task();
    let _ = app.on_event(key(KeyCode::Char('A')));
    assert!(app.prompt_state().is_some());
    type_str(&mut app, "yess");
    // Backspace trims the stray char.
    let _ = app.on_event(key(KeyCode::Backspace));
    assert_eq!(app.prompt_state().unwrap().1, "yes");
    // Left then insert in the middle.
    let _ = app.on_event(key(KeyCode::Left));
    type_str(&mut app, "X");
    assert_eq!(app.prompt_state().unwrap().1, "yeXs");
    // Right + Enter sends and closes the overlay (answer_question is a no-op here).
    let _ = app.on_event(key(KeyCode::Right));
    let cmd = app.on_event(key(KeyCode::Enter));
    assert!(cmd.is_none());
    assert!(app.prompt_state().is_none());
    assert!(
        app.status().contains("Answer sent"),
        "status: {}",
        app.status()
    );
}

#[test]
fn prompt_esc_cancels_and_ctrl_c_quits() {
    let (mut app, _rt) = app_with_selected_task();
    let _ = app.on_event(key(KeyCode::Char('A')));
    let _ = app.on_event(key(KeyCode::Esc));
    assert!(app.prompt_state().is_none());
    assert!(app.status().contains("Cancelled"));

    let _ = app.on_event(key(KeyCode::Char('A')));
    assert!(app.prompt_state().is_some());
    let _ = app.on_event(ctrl(KeyCode::Char('c')));
    assert!(app.should_quit);
}

#[test]
fn prompt_empty_answer_is_cancelled() {
    let (mut app, _rt) = app_with_selected_task();
    let _ = app.on_event(key(KeyCode::Char('A')));
    let cmd = app.on_event(key(KeyCode::Enter));
    assert!(cmd.is_none());
    assert!(
        app.status().contains("Answer cancelled"),
        "status: {}",
        app.status()
    );
}

// --- Workers add prompt (empty registry) ------------------------------------

#[test]
fn workers_add_prompt_emits_add_cmd_for_address() {
    let (mut app, _rt) = empty_app();
    tab(&mut app, "Workers");
    let _ = app.on_event(key(KeyCode::Char('a')));
    let (title, _) = app.prompt_state().expect("add prompt open");
    assert!(title.starts_with("Add worker"));
    type_str(&mut app, "host:1234 my label");
    let cmd = app.on_event(key(KeyCode::Enter));
    match cmd {
        Some(Cmd::WorkerOp(op)) => {
            let dbg = format!("{op:?}");
            assert!(dbg.contains("host:1234"));
            assert!(dbg.contains("my label"));
        }
        other => panic!("expected WorkerOp Add, got {other:?}"),
    }
}

#[test]
fn workers_add_prompt_handle_form() {
    let (mut app, _rt) = empty_app();
    tab(&mut app, "Workers");
    let _ = app.on_event(key(KeyCode::Char('a')));
    type_str(&mut app, "@dev-2");
    let cmd = app.on_event(key(KeyCode::Enter));
    match cmd {
        Some(Cmd::WorkerOp(op)) => {
            let dbg = format!("{op:?}");
            assert!(dbg.contains("@dev-2"));
            // Handle form leaves address None.
            assert!(dbg.contains("handle: Some"));
        }
        other => panic!("expected WorkerOp Add, got {other:?}"),
    }
}

#[test]
fn workers_add_prompt_empty_is_cancelled() {
    let (mut app, _rt) = empty_app();
    tab(&mut app, "Workers");
    let _ = app.on_event(key(KeyCode::Char('a')));
    let cmd = app.on_event(key(KeyCode::Enter));
    assert!(cmd.is_none());
    assert!(
        app.status().contains("Add cancelled"),
        "status: {}",
        app.status()
    );
}

#[test]
fn workers_select_and_remove_no_op_when_empty() {
    let (mut app, _rt) = empty_app();
    tab(&mut app, "Workers");
    // No workers → select/remove produce no command.
    assert!(app.on_event(key(KeyCode::Enter)).is_none());
    assert!(app.on_event(key(KeyCode::Char('d'))).is_none());
    // Up/Down clamp harmlessly at 0.
    let _ = app.on_event(key(KeyCode::Down));
    let _ = app.on_event(key(KeyCode::Up));
    assert_eq!(app.worker_index(), 0);
    // The empty-state hint renders.
    let out = render(&mut app, 120, 40);
    assert!(out.contains("No workers registered"));
}

// --- Context navigation & mouse ---------------------------------------------

#[test]
fn context_jk_navigation_and_render() {
    use medulla::runtime::ContextItem;
    let (mut app, _rt) = demo_app();
    app.set_contexts(vec![
        ContextItem {
            ref_: "ctx://task-1/result".into(),
            kind: "task-result".into(),
            bytes: 482,
            content: "first chunk body".into(),
        },
        ContextItem {
            ref_: "ctx://memory/rules".into(),
            kind: "memory".into(),
            bytes: 128,
            content: "second chunk body".into(),
        },
    ]);
    tab(&mut app, "Context");
    let out = render(&mut app, 120, 40);
    assert!(out.contains("Environment ·"));
    // j moves the selection down, k moves it back up (no panic at the edges).
    let _ = app.on_event(key(KeyCode::Char('j')));
    let _ = app.on_event(key(KeyCode::Char('j')));
    let _ = app.on_event(key(KeyCode::Char('k')));
    let out = render(&mut app, 120, 40);
    assert!(out.contains("task-result") || out.contains("memory"));
}

#[test]
fn mouse_click_selects_agent_and_context_rows() {
    let (mut app, _rt) = demo_app();
    tab(&mut app, "Agents");
    let _ = render(&mut app, 120, 40);
    // Click somewhere inside the Agents list column.
    let _ = app.on_event(mouse(MouseEventKind::Down(MouseButton::Left), 5, 5));
    // Selection is clamped to a selectable row; no panic and a render still works.
    let _ = render(&mut app, 120, 40);

    // Scroll wheel on Agents / Trace / Context routes without panicking.
    let _ = app.on_event(mouse(MouseEventKind::ScrollDown, 5, 5));
    let _ = app.on_event(mouse(MouseEventKind::ScrollUp, 5, 5));
    tab(&mut app, "Trace");
    let _ = app.on_event(mouse(MouseEventKind::ScrollDown, 5, 5));
    let _ = app.on_event(mouse(MouseEventKind::ScrollUp, 5, 5));
}

// --- chat composer edits ----------------------------------------------------

#[test]
fn chat_left_right_backspace_and_esc() {
    let (mut app, _rt) = empty_app();
    app.tab_index = 1;
    type_str(&mut app, "abc");
    assert_eq!(app.draft_cursor(), 3);
    let _ = app.on_event(key(KeyCode::Left));
    assert_eq!(app.draft_cursor(), 2);
    let _ = app.on_event(key(KeyCode::Right));
    assert_eq!(app.draft_cursor(), 3);
    let _ = app.on_event(key(KeyCode::Backspace));
    assert_eq!(app.draft_text(), "ab");
    let _ = app.on_event(key(KeyCode::Esc));
    assert_eq!(app.draft_text(), "");
}

// --- overview rendering: opencode third panel -------------------------------

#[test]
fn overview_renders_opencode_panel_without_tinyplace() {
    let rt = Arc::new(MockRuntime::empty());
    let mut l = LoadedConfig::defaults("medulla.tui.json".into());
    l.config.opencode = Some(medulla::config::OpencodeConfig::default());
    let mut app = App::new(rt, l);
    app.tab_index = 0; // Overview
    let out = render(&mut app, 120, 40);
    assert!(out.contains("OpenCode workers"), "opencode third panel");
}

// --- resume picker: modal swallows mouse, ctrl-c quits ----------------------

#[test]
fn resume_modal_swallows_mouse_and_ctrl_c_quits() {
    let (mut app, _rt) = demo_app();
    app.open_resume(vec![medulla::ui::chat_store::MainChatSummary {
        session_id: "s".into(),
        name: "Chat".into(),
        turns: 1,
        thread_count: 1,
        updated_at: "2026-01-01T00:00:00Z".into(),
    }]);
    assert!(app.resume_open());
    // Mouse is swallowed while the modal is open.
    assert!(app
        .on_event(mouse(MouseEventKind::Down(MouseButton::Left), 5, 5))
        .is_none());
    assert!(app.resume_open());
    // Ctrl-C quits from the modal.
    let _ = app.on_event(ctrl(KeyCode::Char('c')));
    assert!(app.should_quit);
}

#[test]
fn open_resume_with_no_chats_sets_status() {
    let (mut app, _rt) = demo_app();
    app.open_resume(Vec::new());
    assert!(!app.resume_open());
    assert!(app.status().contains("No saved chats"));
}

// --- working indicator: single call, tokens ---------------------------------

// --- events_changed seam ----------------------------------------------------

#[test]
fn events_changed_flips_then_settles() {
    let (mut app, rt) = empty_app();
    // First call records the baseline (0 events) → no change reported.
    assert!(!app.events_changed());
    rt.script_event(TuiEvent::Assistant { body: "x".into() });
    app.refresh_snapshot();
    assert!(app.events_changed(), "a new event is a change");
    assert!(!app.events_changed(), "same length settles");
}

// --- tinyplace observation merge --------------------------------------------

#[test]
fn tinyplace_observation_merges_into_snapshot() {
    use medulla::runtime::{AgentDescriptor, AgentPresence, TinyplaceIdentity};
    use medulla::tinyplace_support::service::TinyplaceObservation;
    use std::collections::HashMap;
    use std::sync::{Arc, Mutex};

    let (mut app, _rt) = empty_app();
    let mut meta = serde_json::Map::new();
    meta.insert("harness".into(), serde_json::json!("tinyplace"));
    let mut presence = HashMap::new();
    presence.insert(
        "peer-1".into(),
        AgentPresence {
            online: true,
            detail: Some("idle".into()),
            at: 1,
        },
    );
    let obs = TinyplaceObservation {
        identity: Some(TinyplaceIdentity {
            agent_id: "cid-xyz".into(),
            public_key: "pk".into(),
            handle: Some("@merged".into()),
        }),
        roster: vec![AgentDescriptor {
            id: "peer-1".into(),
            name: "peer-1".into(),
            description: "a peer".into(),
            availability: "online".into(),
            tags: vec![],
            metadata: meta,
        }],
        presence,
    };
    app.set_tinyplace_observation(Arc::new(Mutex::new(obs)));
    assert!(app.snapshot.tinyplace.is_some());
    assert!(app.snapshot.roster.iter().any(|a| a.id == "peer-1"));
    assert!(app.snapshot.presence.contains_key("peer-1"));
    // The Overview 'me' line reflects the merged handle.
    app.tab_index = 0;
    let out = render(&mut app, 120, 40);
    assert!(out.contains("@merged"), "merged identity should render");
}

// --- chat transcript folding: error + wrapped multi-line turns --------------

#[test]
fn chat_renders_error_and_wrapped_turns() {
    let (mut app, rt) = empty_app();
    let long = "word ".repeat(40);
    rt.script_event(TuiEvent::User { body: long.clone() });
    rt.script_event(TuiEvent::Assistant { body: long });
    rt.script_event(TuiEvent::Error {
        source: "cycle".into(),
        message: "it broke".into(),
    });
    app.refresh_snapshot();
    tab(&mut app, "Chat");
    // Render narrow to force wrapping across multiple rows.
    let out = render(&mut app, 60, 24);
    assert!(out.contains("cycle: it broke"), "error line renders");
    assert!(out.contains("word"), "wrapped body renders");
}

// --- chat thinking spinner --------------------------------------------------

#[test]
fn chat_shows_thinking_spinner_with_and_without_calls() {
    let (mut app, rt) = empty_app();
    rt.set_running(true);
    app.refresh_snapshot();
    tab(&mut app, "Chat");
    // No inference in flight → "working…".
    let out = render(&mut app, 120, 40);
    assert!(out.contains("working"), "idle-stream spinner: {out:.0}");

    rt.script_event(TuiEvent::InferenceStart {
        tier: "reasoning".into(),
        op: "step".into(),
        model: Some("m".into()),
    });
    app.refresh_snapshot();
    let out = render(&mut app, 120, 40);
    assert!(out.contains("model call"), "in-flight spinner detail");
}

// --- thread badges & fork indentation ---------------------------------------

#[test]
fn chat_thread_sidebar_shows_badges_and_indent() {
    let (mut app, rt) = demo_app();
    // Fork so a child thread renders one level deep (⑃ indent).
    rt.fork(Some("child".into()));
    // A running task + a pending question on the child drives the badges.
    rt.script_event(TuiEvent::TaskStart {
        task_id: "cyc-1/t:t9".into(),
        instruction: "go".into(),
        depth: 2,
        agent_id: Some("dev-1".into()),
    });
    rt.script_event(TuiEvent::TaskAttention {
        task_id: "cyc-1/t:t9".into(),
        reason: "confirm".into(),
        content: "?".into(),
        question_id: Some("q".into()),
    });
    app.refresh_snapshot();
    tab(&mut app, "Chat");
    let out = render(&mut app, 120, 40);
    assert!(out.contains("run"), "running-task badge");
    assert!(out.contains('⚠'), "attention badge");
    assert!(out.contains('⑃'), "fork indent glyph");
}

// --- Context mouse scroll ----------------------------------------------------

#[test]
fn context_mouse_wheel_scrolls() {
    use medulla::runtime::ContextItem;
    let (mut app, _rt) = demo_app();
    app.set_contexts(vec![
        ContextItem {
            ref_: "a".into(),
            kind: "memory".into(),
            bytes: 1,
            content: "one".into(),
        },
        ContextItem {
            ref_: "b".into(),
            kind: "memory".into(),
            bytes: 1,
            content: "two".into(),
        },
    ]);
    tab(&mut app, "Context");
    let _ = render(&mut app, 120, 40);
    let _ = app.on_event(mouse(MouseEventKind::ScrollDown, 5, 5));
    let _ = app.on_event(mouse(MouseEventKind::ScrollDown, 5, 5));
    let _ = app.on_event(mouse(MouseEventKind::ScrollUp, 5, 5));
    // No panic; a render still succeeds.
    let _ = render(&mut app, 120, 40);
}

// --- mouse clicks: context row, chat thread, tab-bar into Context -----------

#[test]
fn click_context_tab_requests_inspect() {
    let (mut app, _rt) = demo_app();
    let _ = render(&mut app, 120, 40);
    // The tab bar sits on row 1; the Context label is the 6th tab. Walk columns
    // until a click yields the InspectContext command.
    let mut got = false;
    for x in 0..120u16 {
        if let Some(Cmd::InspectContext) =
            app.on_event(mouse(MouseEventKind::Down(MouseButton::Left), x, 1))
        {
            got = true;
            break;
        }
    }
    assert!(got, "clicking the Context tab requests an inspect");
    assert_eq!(app.tab(), "Context");
}

#[test]
fn click_chat_thread_switches_active() {
    let (mut app, rt) = demo_app();
    rt.fork(Some("branch".into()));
    app.refresh_snapshot();
    tab(&mut app, "Chat");
    let _ = render(&mut app, 120, 40);
    // Click rows inside the threads sidebar (left column, content starts ~row 3).
    for y in 3..8u16 {
        let _ = app.on_event(mouse(MouseEventKind::Down(MouseButton::Left), 3, y));
    }
    // The runtime recorded at least one active-thread switch.
    assert!(rt.recorded_calls().iter().any(|c| c == "set_active_thread"));
}

// --- resume picker navigation -----------------------------------------------

#[test]
fn resume_picker_navigates_and_loads() {
    let (mut app, _rt) = demo_app();
    app.open_resume(vec![
        medulla::ui::chat_store::MainChatSummary {
            session_id: "s1".into(),
            name: "First".into(),
            turns: 1,
            thread_count: 1,
            updated_at: "2026-01-01".into(),
        },
        medulla::ui::chat_store::MainChatSummary {
            session_id: "s2".into(),
            name: "Second".into(),
            turns: 2,
            thread_count: 2,
            updated_at: "2026-01-02".into(),
        },
    ]);
    // Render the modal (Chat tab hosts it in the composer slot).
    tab(&mut app, "Chat");
    let out = render(&mut app, 120, 40);
    assert!(out.contains("Resume a chat"), "modal renders");
    // Down to the second row, back up, down again, then Enter loads it.
    let _ = app.on_event(key(KeyCode::Down));
    let _ = app.on_event(key(KeyCode::Up));
    let _ = app.on_event(key(KeyCode::Down));
    let cmd = app.on_event(key(KeyCode::Enter));
    match cmd {
        Some(Cmd::Resume(id)) => assert_eq!(id, "s2"),
        other => panic!("expected Resume(s2), got {other:?}"),
    }
    assert!(!app.resume_open(), "Enter closes the picker");
}

// --- global control chords --------------------------------------------------

#[test]
fn control_chords_route() {
    let (mut app, rt) = demo_app();
    tab(&mut app, "Chat");
    // Ctrl-O toggles mouse capture (and back).
    let before = app.mouse_capture;
    let _ = app.on_event(ctrl(KeyCode::Char('o')));
    assert_ne!(app.mouse_capture, before);
    let _ = app.on_event(ctrl(KeyCode::Char('o')));
    assert_eq!(app.mouse_capture, before);
    // Ctrl-Y copies the whole chat into the captured sink.
    let sink = app.capture_clipboard();
    let _ = app.on_event(ctrl(KeyCode::Char('y')));
    assert_eq!(sink.lock().unwrap().len(), 1);
    // Ctrl-X aborts, Ctrl-N starts a fresh session.
    let _ = app.on_event(ctrl(KeyCode::Char('x')));
    assert!(app.status().contains("Abort"));
    let _ = app.on_event(ctrl(KeyCode::Char('n')));
    assert!(app.status().contains("fresh"));
    let calls = rt.recorded_calls();
    assert!(calls.iter().any(|c| c == "abort"));
    assert!(calls.iter().any(|c| c == "new_session"));
}

#[test]
fn ctrl_f_forks_and_focuses_chat() {
    let (mut app, rt) = demo_app();
    tab(&mut app, "Agents");
    let _ = app.on_event(ctrl(KeyCode::Char('f')));
    assert_eq!(app.tab(), "Chat");
    assert!(rt.recorded_calls().iter().any(|c| c == "fork"));
}

#[test]
fn ctrl_updown_switches_threads_on_chat() {
    let (mut app, rt) = demo_app();
    rt.fork(Some("branch".into()));
    app.refresh_snapshot();
    tab(&mut app, "Chat");
    let _ = app.on_event(ctrl(KeyCode::Up));
    let _ = app.on_event(ctrl(KeyCode::Down));
    assert!(rt.recorded_calls().iter().any(|c| c == "set_active_thread"));
}

// --- Agents j/k scroll & agent-index navigation -----------------------------

#[test]
fn agents_jk_scroll_and_arrow_nav() {
    let (mut app, _rt) = demo_app();
    tab(&mut app, "Agents");
    let _ = render(&mut app, 120, 40);
    let _ = app.on_event(key(KeyCode::Char('j')));
    let _ = app.on_event(key(KeyCode::Char('j')));
    let _ = app.on_event(key(KeyCode::Char('k')));
    // Arrow up/down move the agent cursor across selectable rows without panic.
    for _ in 0..15 {
        let _ = app.on_event(key(KeyCode::Down));
    }
    for _ in 0..15 {
        let _ = app.on_event(key(KeyCode::Up));
    }
    let _ = render(&mut app, 120, 40);
}

// --- prompt-history recall on the composer ----------------------------------

#[test]
fn up_down_recall_prompt_history() {
    let (mut app, _rt) = empty_app();
    // Build two history entries.
    let _ = submit_line(&mut app, "first prompt");
    let _ = submit_line(&mut app, "second prompt");
    assert_eq!(app.draft_text(), "");
    // Up recalls the most recent, another Up the older.
    let _ = app.on_event(key(KeyCode::Up));
    assert_eq!(app.draft_text(), "second prompt");
    let _ = app.on_event(key(KeyCode::Up));
    assert_eq!(app.draft_text(), "first prompt");
    // Down walks back toward the newest, then to an empty draft.
    let _ = app.on_event(key(KeyCode::Down));
    assert_eq!(app.draft_text(), "second prompt");
    let _ = app.on_event(key(KeyCode::Down));
    assert_eq!(app.draft_text(), "");
}

// --- multi-line caret walk + composer render --------------------------------

#[test]
fn multiline_draft_caret_walk_and_render() {
    let (mut app, _rt) = empty_app();
    tab(&mut app, "Chat");
    type_str(&mut app, "line one");
    let _ = app.on_event(Event::Key(KeyEvent::new(
        KeyCode::Enter,
        KeyModifiers::SHIFT,
    )));
    type_str(&mut app, "line two");
    assert!(app.draft_text().contains('\n'));
    // Up moves the caret to the first row (not history, since we're multi-line).
    let _ = app.on_event(key(KeyCode::Up));
    let _ = app.on_event(key(KeyCode::Down));
    // The composer renders both rows with the reversed caret cell.
    let out = render(&mut app, 120, 40);
    assert!(out.contains("line one") && out.contains("line two"));
}

// --- cancel with a cycle-less task id ---------------------------------------

#[test]
fn cancel_task_without_cycle_prefix_reports_no_cycle() {
    let (mut app, rt) = demo_app();
    // A bare task id (no `/t:` cycle prefix) yields a Sub row with no cycle.
    rt.script_event(TuiEvent::TaskStart {
        task_id: "bare-task".into(),
        instruction: "go".into(),
        depth: 2,
        agent_id: Some("dev-1".into()),
    });
    app.refresh_snapshot();
    tab(&mut app, "Agents");
    for _ in 0..14 {
        if app.selected_task_id().as_deref() == Some("bare-task") {
            break;
        }
        let _ = app.on_event(key(KeyCode::Down));
    }
    if app.selected_task_id().as_deref() == Some("bare-task") {
        let _ = app.on_event(key(KeyCode::Char('X')));
        assert!(
            app.status().contains("no cycle"),
            "status: {}",
            app.status()
        );
    }
}

// --- Trace tab renders the JSON detail row ----------------------------------

#[test]
fn trace_tab_renders_event_and_json() {
    use medulla::ui::events::NodeTrace;
    let (mut app, rt) = empty_app();
    rt.script_event(TuiEvent::Trace {
        entry: NodeTrace {
            node: "orchestrate".into(),
            ms: 42,
            tool: None,
            op: Some("decide".into()),
        },
    });
    app.refresh_snapshot();
    tab(&mut app, "Trace");
    let out = render(&mut app, 120, 40);
    assert!(out.contains("Trace ·"), "trace header");
    assert!(out.contains("orchestrate"), "trace json detail row");
}

// --- Workers registry with a harness + stream-health header -----------------

#[test]
fn workers_render_with_harness_and_stream_health() {
    let mut app = fleet_app();
    tab(&mut app, "Workers");
    let out = render(&mut app, 120, 40);
    assert!(out.contains("CLAUDE"), "worker harness badge upper-cased");
    assert!(out.contains("primary"), "worker label renders");

    // The header shows stream health when a cycle runs under a stream-tracking runtime.
    tab(&mut app, "Overview");
    let out = render(&mut app, 120, 40);
    assert!(
        out.contains("live"),
        "stream-state label in header: {out:.0}"
    );
}

#[test]
fn overview_shows_active_model_calls_and_completed_task() {
    let (mut app, rt) = empty_app();
    rt.script_event(TuiEvent::InferenceStart {
        tier: "reasoning".into(),
        op: "step".into(),
        model: Some("m".into()),
    });
    rt.script_event(TuiEvent::TaskComplete {
        digest: TaskDigest {
            task_id: "t1".into(),
            status: "done".into(),
            digest: "d".into(),
            result_ref: None,
            usage: Some(Usage {
                input_tokens: 10,
                output_tokens: 2,
            }),
            depth: 2,
        },
    });
    app.refresh_snapshot();
    app.tab_index = 0;
    let out = render(&mut app, 120, 40);
    assert!(
        out.contains("active model calls 1"),
        "overview: active calls"
    );
}
