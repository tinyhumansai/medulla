//! Feature-level tests: drive the `App` like a user via synthetic crossterm
//! events and assert on observable state and the rendered `TestBackend` buffer.
//! These complement the crate's inline unit tests — here we exercise whole flows
//! (typing + submit, slash commands, tab nav, scrolling, working indicator,
//! resume picker, threads/fork, abort/new-session, copy, config rendering).

use std::sync::Arc;

use crossterm::event::{
    Event, KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind,
};
use ratatui::backend::TestBackend;
use ratatui::Terminal;

use medulla::config::{LoadedConfig, TinyplaceConfig};
use medulla::runtime::mock::MockRuntime;
use medulla::ui::app::{App, Cmd, TABS};
use medulla::ui::chat_store::MainChatSummary;
use medulla::ui::events::TuiEvent;

// --- harness helpers --------------------------------------------------------

fn loaded() -> LoadedConfig {
    let mut l = LoadedConfig::defaults("medulla.tui.json".into());
    l.config.tinyplace = Some(TinyplaceConfig::default());
    l
}

/// App over a populated demo runtime; returns the app and the concrete handle so
/// tests can script events / read recorded calls.
fn demo_app() -> (App, Arc<MockRuntime>) {
    let rt = Arc::new(MockRuntime::demo());
    let app = App::new(rt.clone(), loaded());
    (app, rt)
}

/// App over a bare runtime (no roster, empty chat).
fn empty_app() -> (App, Arc<MockRuntime>) {
    let rt = Arc::new(MockRuntime::empty());
    let app = App::new(rt.clone(), loaded());
    (app, rt)
}

fn key(code: KeyCode) -> Event {
    Event::Key(KeyEvent::new(code, KeyModifiers::NONE))
}

fn key_mod(code: KeyCode, mods: KeyModifiers) -> Event {
    Event::Key(KeyEvent::new(code, mods))
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

/// Compose `s` on the Chat tab and press Enter, returning the resulting `Cmd`.
fn submit_line(app: &mut App, s: &str) -> Option<Cmd> {
    app.tab_index = 1;
    type_str(app, s);
    app.on_event(key(KeyCode::Enter))
}

/// Draw once into a fresh backend and flatten the buffer into a string.
fn render(app: &mut App, w: u16, h: u16) -> String {
    let mut terminal = Terminal::new(TestBackend::new(w, h)).unwrap();
    terminal.draw(|f| app.draw(f)).unwrap();
    let buf = terminal.backend().buffer().clone();
    buf.content().iter().map(|c| c.symbol()).collect()
}

// --- 1. typing + submit flow ------------------------------------------------

#[test]
fn typing_then_enter_emits_submit_and_clears_draft() {
    let (mut app, _rt) = empty_app();
    app.tab_index = 1;
    type_str(&mut app, "hello world");
    assert_eq!(app.draft_text(), "hello world");
    let cmd = app.on_event(key(KeyCode::Enter));
    assert!(matches!(cmd, Some(Cmd::Submit(s)) if s == "hello world"));
    assert_eq!(app.draft_text(), "");
    assert_eq!(app.status(), "Cycle running…");
}

#[test]
fn up_down_recall_prompt_history() {
    let (mut app, _rt) = empty_app();
    let _ = submit_line(&mut app, "hello");
    assert_eq!(app.draft_text(), "");
    // Up recalls the last submitted line; Down returns to a fresh draft.
    let _ = app.on_event(key(KeyCode::Up));
    assert_eq!(app.draft_text(), "hello");
    let _ = app.on_event(key(KeyCode::Down));
    assert_eq!(app.draft_text(), "");
}

#[test]
fn shift_enter_inserts_newline_and_esc_clears() {
    let (mut app, _rt) = empty_app();
    app.tab_index = 1;
    type_str(&mut app, "ab");
    let _ = app.on_event(key_mod(KeyCode::Enter, KeyModifiers::SHIFT));
    assert_eq!(app.draft_text(), "ab\n");
    type_str(&mut app, "cd");
    assert_eq!(app.draft_text(), "ab\ncd");
    let _ = app.on_event(key(KeyCode::Esc));
    assert_eq!(app.draft_text(), "");
}

#[test]
fn multiline_caret_walks_rows_before_history() {
    let (mut app, _rt) = empty_app();
    // Prime history so the fallback has something to recall.
    let _ = submit_line(&mut app, "prev");
    // Build a two-line draft "a\nb" with the caret at the end.
    type_str(&mut app, "a");
    let _ = app.on_event(key_mod(KeyCode::Enter, KeyModifiers::SHIFT));
    type_str(&mut app, "b");
    assert_eq!(app.draft_text(), "a\nb");
    // First Up moves the caret up a row; the text is unchanged (not history).
    let _ = app.on_event(key(KeyCode::Up));
    assert_eq!(app.draft_text(), "a\nb");
    assert_eq!(app.draft_cursor(), 1);
    // Second Up is at the top row → falls back to prompt-history recall.
    let _ = app.on_event(key(KeyCode::Up));
    assert_eq!(app.draft_text(), "prev");
}

// --- 2. slash commands ------------------------------------------------------

#[test]
fn slash_help_and_config_switch_tabs() {
    let (mut app, _rt) = empty_app();
    let _ = submit_line(&mut app, "/help");
    assert_eq!(app.tab(), "Help");
    let _ = submit_line(&mut app, "/config");
    assert_eq!(app.tab(), "Config");
}

#[test]
fn slash_async_toggles_flag() {
    let (mut app, _rt) = empty_app();
    assert!(!app.snapshot.async_mode);
    let _ = submit_line(&mut app, "/async");
    assert!(app.snapshot.async_mode);
    let _ = submit_line(&mut app, "/async");
    assert!(!app.snapshot.async_mode);
}

#[test]
fn slash_copy_empty_chat_reports_nothing_to_copy() {
    let (mut app, _rt) = empty_app();
    let _ = submit_line(&mut app, "/copy");
    assert!(
        app.status().contains("Nothing to copy"),
        "status: {}",
        app.status()
    );
}

#[test]
fn slash_mouse_toggles_capture_flag() {
    let (mut app, _rt) = empty_app();
    assert!(app.mouse_capture);
    let _ = submit_line(&mut app, "/mouse");
    assert!(!app.mouse_capture);
}

#[test]
fn unknown_slash_command_sets_status() {
    let (mut app, _rt) = empty_app();
    let _ = submit_line(&mut app, "/bogus");
    assert!(
        app.status().contains("Unknown command"),
        "status: {}",
        app.status()
    );
}

#[test]
fn slash_quit_sets_should_quit() {
    let (mut app, _rt) = empty_app();
    assert!(!app.should_quit);
    let _ = submit_line(&mut app, "/quit");
    assert!(app.should_quit);
}

// --- 3. tab navigation ------------------------------------------------------

#[test]
fn tab_and_backtab_cycle_tabs() {
    let (mut app, _rt) = demo_app();
    assert_eq!(app.tab(), "Overview");
    let _ = app.on_event(key(KeyCode::Tab));
    assert_eq!(app.tab(), "Chat");
    let _ = app.on_event(key(KeyCode::Tab));
    assert_eq!(app.tab(), "Agents");
    let _ = app.on_event(key_mod(KeyCode::BackTab, KeyModifiers::SHIFT));
    assert_eq!(app.tab(), "Chat");
    // Wrap backwards from Overview to Help.
    let _ = app.on_event(key_mod(KeyCode::BackTab, KeyModifiers::SHIFT));
    let _ = app.on_event(key_mod(KeyCode::BackTab, KeyModifiers::SHIFT));
    assert_eq!(app.tab(), "Help");
}

#[test]
fn clicking_tab_bar_selects_tab() {
    let (mut app, _rt) = demo_app();
    // Draw first so the tab hit-boxes are recorded, then click within "Agents".
    let _ = render(&mut app, 120, 40);
    let _ = app.on_event(mouse(MouseEventKind::Down(MouseButton::Left), 20, 1));
    assert_eq!(app.tab(), "Agents");
}

#[test]
fn each_tab_renders_its_signature() {
    let signatures = [
        ("Chat", "Threads"),
        ("Agents", "Agents ·"),
        ("Trace", "Trace ·"),
        ("Context", "Environment ·"),
        ("Config", "Configuration ·"),
    ];
    for (name, sig) in signatures {
        let (mut app, _rt) = demo_app();
        app.tab_index = TABS.iter().position(|t| *t == name).unwrap();
        let out = render(&mut app, 120, 40);
        assert!(out.contains("MEDULLA LAB"), "{name}: missing header");
        assert!(out.contains(sig), "{name}: missing signature {sig:?}");
    }
}

// --- 4. chat scroll behavior ------------------------------------------------

fn script_many_chat(rt: &Arc<MockRuntime>, n: usize) {
    for i in 0..n {
        rt.script_event(TuiEvent::Assistant {
            body: format!("reply {i}"),
        });
    }
}

#[test]
fn page_up_scrolls_and_page_down_returns_to_bottom() {
    let (mut app, rt) = empty_app();
    script_many_chat(&rt, 40);
    app.refresh_snapshot();
    app.tab_index = 1;
    // Prime area geometry.
    let _ = render(&mut app, 80, 24);
    assert_eq!(app.chat_scroll(), 0);

    let _ = app.on_event(key(KeyCode::PageUp));
    let out = render(&mut app, 80, 24);
    assert!(
        app.chat_scroll() > 0,
        "PageUp should grow the scroll offset"
    );
    assert!(
        out.contains("line(s) below"),
        "expected a below-fold notice"
    );

    let _ = app.on_event(key(KeyCode::PageDown));
    let _ = render(&mut app, 80, 24);
    assert_eq!(app.chat_scroll(), 0, "PageDown should return to the bottom");
}

#[test]
fn wheel_scroll_adjusts_offset_by_three() {
    let (mut app, rt) = empty_app();
    script_many_chat(&rt, 40);
    app.refresh_snapshot();
    app.tab_index = 1;
    let _ = render(&mut app, 80, 24);

    let _ = app.on_event(mouse(MouseEventKind::ScrollUp, 10, 10));
    assert_eq!(app.chat_scroll(), 3);
    let _ = app.on_event(mouse(MouseEventKind::ScrollDown, 10, 10));
    assert_eq!(app.chat_scroll(), 0);
}

// --- 5. working indicator ---------------------------------------------------

#[test]
fn inference_start_shows_working_then_cycle_end_idles() {
    let (mut app, rt) = empty_app();
    rt.script_event(TuiEvent::InferenceStart {
        tier: "reasoning".into(),
        op: "execute_step".into(),
        model: Some("gpt-4o".into()),
    });
    rt.script_event(TuiEvent::InferenceStart {
        tier: "reasoning".into(),
        op: "execute_step".into(),
        model: Some("gpt-4o".into()),
    });
    rt.set_running(true);
    app.refresh_snapshot();
    app.tab_index = 1;
    let out = render(&mut app, 100, 30);
    assert!(out.contains("thinking"), "expected a working indicator");
    assert!(
        out.contains("in flight"),
        "expected model-call-in-flight notice"
    );

    // Cycle ends → idle: the working notice is gone.
    rt.set_running(false);
    rt.script_event(TuiEvent::CycleEnd {
        cycle_id: "cyc-1".into(),
        pass_count: 1,
        duration_ms: 10,
    });
    app.refresh_snapshot();
    let out = render(&mut app, 100, 30);
    assert!(
        !out.contains("in flight"),
        "idle should drop the in-flight notice"
    );
}

// --- 6. resume picker -------------------------------------------------------

fn sample_chats() -> Vec<MainChatSummary> {
    (0..3)
        .map(|i| MainChatSummary {
            session_id: format!("sess-{i}"),
            name: format!("Chat {i}"),
            turns: 2,
            thread_count: 1,
            updated_at: "2026-01-01T00:00:00Z".into(),
        })
        .collect()
}

#[test]
fn resume_picker_navigates_and_enter_resumes() {
    let (mut app, _rt) = demo_app();
    app.open_resume(sample_chats());
    assert!(app.resume_open());
    // Down once, then Enter → resume the second chat.
    let _ = app.on_event(key(KeyCode::Down));
    let cmd = app.on_event(key(KeyCode::Enter));
    assert!(
        matches!(&cmd, Some(Cmd::Resume(id)) if id == "sess-1"),
        "cmd: {cmd:?}"
    );
    assert!(!app.resume_open(), "Enter should close the picker");
}

#[test]
fn resume_picker_esc_closes_without_resuming() {
    let (mut app, _rt) = demo_app();
    app.open_resume(sample_chats());
    assert!(app.resume_open());
    let cmd = app.on_event(key(KeyCode::Esc));
    assert!(cmd.is_none());
    assert!(!app.resume_open());
}

// --- 7. thread / fork UX ----------------------------------------------------

#[test]
fn ctrl_f_forks_thread_and_focuses_chat() {
    let (mut app, rt) = demo_app();
    assert_eq!(app.snapshot.threads.len(), 1);
    let _ = app.on_event(ctrl(KeyCode::Char('f')));
    assert_eq!(app.tab(), "Chat", "fork should focus the Chat tab");
    assert_eq!(app.snapshot.threads.len(), 2, "fork should add a thread");
    assert!(rt.recorded_calls().iter().any(|c| c == "fork"));
}

#[test]
fn ctrl_up_down_switches_threads() {
    let (mut app, _rt) = demo_app();
    // Fork to create a second thread; the fork becomes active.
    let _ = app.on_event(ctrl(KeyCode::Char('f')));
    let active_after_fork = app.snapshot.active_thread_id.clone();
    // Ctrl-Up moves to the previous (parent) thread.
    let _ = app.on_event(ctrl(KeyCode::Up));
    assert_eq!(app.snapshot.active_thread_id, "t1");
    // Ctrl-Down returns to the forked thread.
    let _ = app.on_event(ctrl(KeyCode::Down));
    assert_eq!(app.snapshot.active_thread_id, active_after_fork);
}

// --- 8. abort / new-session keys -------------------------------------------

#[test]
fn ctrl_x_aborts_and_ctrl_n_starts_new_session() {
    let (mut app, rt) = demo_app();
    let _ = app.on_event(ctrl(KeyCode::Char('x')));
    assert!(app.status().contains("Abort"), "status: {}", app.status());

    let _ = app.on_event(ctrl(KeyCode::Char('n')));
    assert!(app.status().contains("fresh"), "status: {}", app.status());

    let calls = rt.recorded_calls();
    assert!(calls.iter().any(|c| c == "abort"), "calls: {calls:?}");
    assert!(calls.iter().any(|c| c == "new_session"), "calls: {calls:?}");
}

// --- 9. copy ----------------------------------------------------------------

#[test]
fn ctrl_y_copies_transcript_via_captured_clipboard() {
    let (mut app, rt) = empty_app();
    rt.script_event(TuiEvent::User {
        body: "hello".into(),
    });
    rt.script_event(TuiEvent::Assistant {
        body: "hi there".into(),
    });
    app.refresh_snapshot();
    let sink = app.capture_clipboard();

    let _ = app.on_event(ctrl(KeyCode::Char('y')));
    let captured = sink.lock().unwrap();
    assert_eq!(captured.len(), 1, "one copy should have been recorded");
    let text = &captured[0];
    assert!(text.contains("> hello"), "transcript: {text:?}");
    assert!(text.contains("hi there"), "transcript: {text:?}");
    assert!(app.status().contains("chars"), "status: {}", app.status());
}

// --- 10. config rendering ---------------------------------------------------

#[test]
fn config_tab_annotates_api_key_env_presence() {
    // Present env → "(set)".
    let set_var = "MEDULLA_FEATURE_TEST_KEY_SET";
    std::env::set_var(set_var, "x");
    let (mut app, _rt) = empty_app();
    app.loaded.config.inference.api_key_env = set_var.into();
    app.tab_index = TABS.iter().position(|t| *t == "Config").unwrap();
    let out = render(&mut app, 200, 50);
    assert!(out.contains("(set)"), "expected (set) annotation");
    std::env::remove_var(set_var);

    // Absent env → "(missing)".
    let missing_var = "MEDULLA_FEATURE_TEST_KEY_MISSING";
    std::env::remove_var(missing_var);
    let (mut app, _rt) = empty_app();
    app.loaded.config.inference.api_key_env = missing_var.into();
    app.tab_index = TABS.iter().position(|t| *t == "Config").unwrap();
    let out = render(&mut app, 200, 50);
    assert!(out.contains("(missing)"), "expected (missing) annotation");
}
