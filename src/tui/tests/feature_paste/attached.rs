//! Paste with a harness attached, which is a second keyboard target: the pane
//! *is* the operator's terminal then, so the payload has to reach the child
//! rather than the composer sitting invisible behind it.
//!
//! Unix-only: it runs `/bin/sh` on a real pty, which Windows has no equivalent
//! of.

#![cfg(unix)]

use crate::helpers::*;

use std::collections::HashMap;
use std::time::{Duration, Instant};

use ratatui::backend::TestBackend;
use ratatui::Terminal;

use medulla::protocol::HarnessProvider;
use medulla_tui::ui::harness_pane::LocalSessions;
use medulla_tui::worker::pty::{LaunchSpec, PtyManager, SessionControl};

/// A shell on a pty running `script`.
///
/// Every script here ends in `read`, which is the point: it answers only
/// once the line is terminated, so the assertions distinguish "the paste
/// arrived" from "the paste arrived as text the child is still waiting to
/// see the end of".
fn shell_session(sessions: &PtyManager, script: &str) -> String {
    let mut env = HashMap::new();
    if let Ok(path) = std::env::var("PATH") {
        env.insert("PATH".to_string(), path);
    }
    env.insert("TERM".to_string(), "xterm-256color".to_string());
    sessions
        .open(LaunchSpec {
            provider: HarnessProvider::Codex,
            bin: "/bin/sh".to_string(),
            cwd: "/".to_string(),
            env,
            extra_args: vec!["-c".to_string(), script.to_string()],
            skip_permissions: false,
            label: "you:codex".to_string(),
            model: None,
            session_id: None,
            control: SessionControl::User,
            origin: medulla_tui::worker::pty::SessionOrigin::User,
            name: None,
            mcp_grant_session: None,
        })
        .expect("the shell opens on a pty")
}

/// An Agents-tab app attached to `id`, reached the way the operator does:
/// move the cursor onto the harness row, then press the focus chord.
fn attached_app(sessions: PtyManager, id: &str) -> App {
    let mut app = agents_app();
    app.set_local_sessions(LocalSessions {
        hooks: medulla::harness_hooks::HooksConfig::default(),
        log: None,
        sessions,
        runtimes: Arc::new(std::sync::Mutex::new(Vec::new())),
        hub_address: "medulla-orchestrator".to_string(),
        env: HashMap::new(),
        workspace: "/".to_string(),
        providers: vec![HarnessProvider::Codex],
        custom_harnesses: Vec::new(),
        router: None,
        attribution: true,
    });

    for _ in 0..64 {
        draw(&mut app);
        if app.pane_session_for_test().is_some() {
            break;
        }
        let _ = app.on_event(Event::Key(KeyEvent::new(KeyCode::Down, KeyModifiers::ALT)));
    }
    draw(&mut app);
    let _ = app.on_event(Event::Key(KeyEvent::new(
        KeyCode::Char(']'),
        KeyModifiers::CONTROL,
    )));
    assert_eq!(app.attached_session(), Some(id), "{}", app.status());
    app
}

/// Render once; the pane resolves its session on the draw, not on the event.
fn draw(app: &mut App) {
    let mut terminal = Terminal::new(TestBackend::new(140, 44)).unwrap();
    terminal.draw(|f| app.draw(f)).unwrap();
}

/// Spin until `check` passes — a real child on a real pty is at the mercy of
/// machine load.
fn wait_for(what: &str, mut check: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(30);
    while Instant::now() < deadline {
        if check() {
            return;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    panic!("timed out waiting for {what}");
}

/// The script every test here runs unless it needs the mode set: read a
/// line and say what it read.
const READ_A_LINE: &str = "read line; printf 'typed:%s\\n' \"$line\"; sleep 30";

#[test]
fn a_paste_goes_to_the_attached_harness_rather_than_the_composer() {
    let sessions = PtyManager::new();
    let id = shell_session(&sessions, READ_A_LINE);
    let mut app = attached_app(sessions.clone(), &id);

    // `/bin/sh` never asks for bracketed paste, so the payload goes as bare
    // bytes with its break as the carriage return a raw line discipline
    // reads as the end of a line — markers sent here would arrive as an
    // escape key press followed by literal text.
    assert!(paste(&mut app, "hello world\n").is_none());

    wait_for("the harness to act on what was pasted", || {
        let mut screen = String::new();
        sessions.screen_text_into(&id, &mut screen);
        screen.contains("typed:hello world")
    });
    assert_eq!(
        app.draft_text(),
        "",
        "nothing leaked into the composer behind the pane"
    );

    sessions.shutdown();
}

#[test]
fn a_child_that_asked_for_bracketed_paste_gets_the_markers() {
    // The case a real harness is in: Claude Code and Codex both enable the
    // mode, and it is what makes a multi-line clipboard land in their
    // composer as one block instead of submitting itself line by line.
    // Asserted on the bytes the child *read*, not on the screen — our own
    // emulator swallows the escapes on echo, so a screen-level check would
    // pass either way.
    let sessions = PtyManager::new();
    let id = shell_session(
        &sessions,
        "printf '\\033[?2004h'; read line; \
         case $line in *200~*) echo BRACKETED;; *) echo BARE;; esac; sleep 30",
    );
    wait_for("the child to enable bracketed paste", || {
        sessions.bracketed_paste(&id) == Some(true)
    });
    let mut app = attached_app(sessions.clone(), &id);

    assert!(paste(&mut app, "hello world\n").is_none());

    wait_for("the child to report what it read", || {
        let mut screen = String::new();
        sessions.screen_text_into(&id, &mut screen);
        screen.contains("BRACKETED") || screen.contains("BARE")
    });
    let mut screen = String::new();
    sessions.screen_text_into(&id, &mut screen);
    assert!(
        screen.contains("BRACKETED"),
        "a child in the mode must see the paste wrapped: {screen}"
    );

    sessions.shutdown();
}

#[test]
fn the_handback_question_swallows_the_paste_instead_of_typing_it() {
    let sessions = PtyManager::new();
    let id = shell_session(&sessions, READ_A_LINE);
    let mut app = attached_app(sessions.clone(), &id);

    // Releasing asks who keeps the harness, and asks it while still
    // attached. The chrome holds the keyboard until that is answered, so a
    // paste in the meantime belongs to neither the harness nor the composer.
    let _ = app.on_event(Event::Key(KeyEvent::new(
        KeyCode::Char(']'),
        KeyModifiers::CONTROL,
    )));
    assert_eq!(
        app.attached_session(),
        Some(id.as_str()),
        "the question is asked with the pane still attached"
    );

    assert!(paste(&mut app, "stray text\n").is_none());

    // Answering keeps the harness but returns the keyboard; the shell is
    // still waiting on its `read`, which is how we know nothing was typed
    // into it, and the composer is untouched.
    let _ = app.on_event(key(KeyCode::Char('n')));
    assert_eq!(app.attached_session(), None, "{}", app.status());
    let mut screen = String::new();
    sessions.screen_text_into(&id, &mut screen);
    assert!(
        !screen.contains("typed:"),
        "the harness was not typed into: {screen}"
    );
    assert_eq!(
        app.draft_text(),
        "",
        "and the composer behind the question stayed empty"
    );

    sessions.shutdown();
}

#[test]
fn the_handback_note_takes_a_paste_once_it_is_being_typed() {
    let sessions = PtyManager::new();
    let id = shell_session(&sessions, READ_A_LINE);
    let mut app = attached_app(sessions.clone(), &id);
    let _ = app.on_event(Event::Key(KeyEvent::new(
        KeyCode::Char(']'),
        KeyModifiers::CONTROL,
    )));

    // `E` turns the note into a text input — every key is text from here,
    // which is why `y` and `n` stop answering the question. A paste is text
    // too, and pasting what you were doing into the brief is exactly what
    // the note is for.
    let _ = app.on_event(key(KeyCode::Char('E')));
    assert!(paste(&mut app, "was mid-migration\non the auth tables\n").is_none());

    let mut terminal = Terminal::new(TestBackend::new(140, 44)).expect("test terminal");
    terminal.draw(|f| app.draw(f)).expect("draws");
    let screen: String = terminal
        .backend()
        .buffer()
        .content()
        .iter()
        .map(|cell| cell.symbol())
        .collect();
    // One line, so the breaks flatten to spaces: the note row has no second
    // row to draw them on.
    assert!(
        screen.contains("Note: was mid-migration on the auth tables"),
        "the pasted note is in the field: {screen}"
    );

    // And it is the note that was pasted into, not the harness behind it:
    // the shell is still waiting on its `read`.
    let mut pane = String::new();
    sessions.screen_text_into(&id, &mut pane);
    assert!(
        !pane.contains("typed:"),
        "the harness was untouched: {pane}"
    );

    sessions.shutdown();
}
