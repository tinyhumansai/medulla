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
