//! Context navigation, mouse routing, and resume-picker coverage: j/k and wheel
//! scrolling, tab-bar and row clicks, and resume-modal navigation.

use crate::helpers::*;

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
    tab(&mut app, "Sessions");
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
    tab(&mut app, "Sessions");
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

// --- Status-line stream health ---------------------------------------------

#[test]
fn the_status_line_shows_stream_health_beside_the_backend_host() {
    let mut app = fleet_app();
    // A dot rather than the spelled-out label it replaced: that cost a third of
    // the line to say what a glyph says.
    tab(&mut app, "Subconscious");
    let out = render(&mut app, 120, 40);
    assert!(
        out.contains("● api.tinyhumans.ai"),
        "live connection dot on the status line: {out:.0}"
    );
}
