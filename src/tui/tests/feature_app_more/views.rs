//! Workers-tab, Context navigation, mouse routing, and resume-picker coverage:
//! the add-worker prompt forms, empty-registry no-ops, j/k and wheel scrolling,
//! tab-bar and row clicks, and resume-modal navigation.

use crate::helpers::*;

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

// --- resume picker: modal swallows mouse, ctrl-c quits ----------------------

#[test]
fn resume_modal_swallows_mouse_and_ctrl_c_quits() {
    let (mut app, _rt) = demo_app();
    app.open_resume(vec![medulla_tui::ui::chat_store::MainChatSummary {
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
        medulla_tui::ui::chat_store::MainChatSummary {
            session_id: "s1".into(),
            name: "First".into(),
            turns: 1,
            thread_count: 1,
            updated_at: "2026-01-01".into(),
        },
        medulla_tui::ui::chat_store::MainChatSummary {
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
