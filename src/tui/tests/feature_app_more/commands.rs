//! Command-surface coverage: slash-command variants, Tab-driven Context inspect,
//! composer editing, global control chords, prompt-history recall, and multi-line
//! draft caret walking.

use crate::helpers::*;

// --- slash-command variants -------------------------------------------------

#[test]
fn slash_resume_emits_list_chats_cmd() {
    let (mut app, _rt) = empty_app();
    let cmd = submit_line(&mut app, "/resume");
    assert!(matches!(cmd, Some(Cmd::ListChats)));
}

#[test]
fn slash_review_refuses_silent_self_review() {
    let (mut app, _rt) = demo_app();
    let cmd = submit_line(&mut app, "/review task-1");
    assert!(cmd.is_none());
    assert!(
        app.status().contains("no online agent other than 'dev-1'"),
        "status: {}",
        app.status()
    );
}

#[test]
fn slash_review_requires_a_known_target() {
    let (mut app, _rt) = demo_app();
    let cmd = submit_line(&mut app, "/review missing-task");
    assert!(cmd.is_none());
    assert!(app.status().contains("was not found"));
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

// --- entering the Context subpage requests an inspect ------------------------

#[test]
fn entering_the_context_subpage_requests_inspect() {
    let (mut app, _rt) = demo_app();
    // Context is a Settings subpage now, so entry — not a Tab press — loads it.
    let cmd = app.focus_settings_subpage("Context");
    assert_eq!(app.tab(), "Settings");
    assert_eq!(app.settings_subpage(), "Context");
    assert!(matches!(cmd, Some(Cmd::InspectContext)));
}

#[test]
fn arrow_keys_walk_the_settings_nav_and_load_each_subpage() {
    let (mut app, _rt) = demo_app();
    let _ = app.focus_settings_subpage("Appearance");
    // Arrows only walk the nav from the nav; step out of the content pane first.
    app.on_event(key(KeyCode::Esc));
    assert!(!app.settings_focused());
    // Down from Appearance → Config → Feedback → Trace → Context.
    for expected in ["Config", "Feedback", "Trace", "Context"] {
        let cmd = app.on_event(key(KeyCode::Down));
        assert_eq!(app.settings_subpage(), expected);
        if expected == "Context" {
            assert!(
                matches!(cmd, Some(Cmd::InspectContext)),
                "Context loads on entry"
            );
        }
    }
    let _ = app.on_event(key(KeyCode::Up));
    assert_eq!(app.settings_subpage(), "Trace");
}

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
