//! Bracketed paste into the main TUI.
//!
//! A paste arrives as one `Event::Paste` rather than as a stream of synthetic
//! key presses, so the newlines inside it are text and not `Enter`. These tests
//! pin that down where it used to go wrong: a multi-line paste submitting itself
//! once per line, and a `/`-prefixed paste running whatever the command peek was
//! highlighting. Everything here is state-level — no real terminal is involved,
//! because the terminal mode itself is set once in `TermGuard::setup`.

use std::sync::Arc;

use crossterm::event::{Event, KeyCode, KeyEvent, KeyModifiers};

use medulla::config::{LoadedConfig, TinyplaceConfig};
use medulla::runtime::mock::MockRuntime;
use medulla_tui::ui::app::{App, Cmd, TABS};

fn loaded() -> LoadedConfig {
    let mut l = LoadedConfig::defaults("medulla.tui.json".into());
    l.config.tinyplace = Some(TinyplaceConfig::default());
    l
}

/// An app on the Agents tab, where the chat composer lives.
fn agents_app() -> App {
    let mut app = App::new(Arc::new(MockRuntime::empty()), loaded());
    app.tab_index = tab_index("Agents");
    app
}

fn tab_index(name: &str) -> usize {
    TABS.iter().position(|t| *t == name).unwrap_or(0)
}

fn key(code: KeyCode) -> Event {
    Event::Key(KeyEvent::new(code, KeyModifiers::NONE))
}

fn type_str(app: &mut App, text: &str) {
    for ch in text.chars() {
        let _ = app.on_event(key(KeyCode::Char(ch)));
    }
}

fn paste(app: &mut App, text: &str) -> Option<Cmd> {
    app.on_event(Event::Paste(text.into()))
}

#[test]
fn a_multiline_paste_lands_in_the_draft_instead_of_submitting_itself() {
    let mut app = agents_app();

    // The reported repro: a log excerpt with CRLF line endings and a trailing
    // newline. Every one of those breaks used to fire a separate submit.
    let cmd = paste(&mut app, "first\r\nsecond\rthird\n");

    assert!(cmd.is_none(), "a paste never produces a command");
    assert_eq!(app.draft_text(), "first\nsecond\nthird\n");
    assert_eq!(app.draft_cursor(), 19);
    assert_ne!(app.status(), "Cycle running…", "nothing was sent");
}

#[test]
fn only_an_explicit_enter_submits_what_was_pasted() {
    let mut app = agents_app();
    paste(&mut app, "look at\nthis trace");

    let cmd = app.on_event(key(KeyCode::Enter));

    assert!(
        matches!(cmd, Some(Cmd::Submit(ref s)) if s == "look at\nthis trace"),
        "the whole block goes as one instruction: {cmd:?}"
    );
    assert_eq!(app.draft_text(), "");
}

#[test]
fn a_paste_inserts_at_the_caret_rather_than_at_the_end() {
    let mut app = agents_app();
    type_str(&mut app, "ac");
    let _ = app.on_event(key(KeyCode::Left));

    let cmd = paste(&mut app, "b\n");

    assert!(cmd.is_none());
    assert_eq!(app.draft_text(), "ab\nc");
    assert_eq!(app.draft_cursor(), 3);
}

#[test]
fn a_paste_into_the_command_peek_does_not_run_the_highlighted_command() {
    let mut app = agents_app();
    type_str(&mut app, "/qu");
    assert!(
        app.selected_command().is_some(),
        "the peek is open on a match"
    );

    // `/qu` alone is not a command, but Enter here would have run `/quit`.
    let cmd = paste(&mut app, "estion for you\n");

    assert!(cmd.is_none());
    assert!(!app.should_quit, "the peeked command did not run");
    assert_eq!(app.draft_text(), "/question for you\n");
}

#[test]
fn a_paste_that_opens_the_peek_resets_its_cursor() {
    let mut app = agents_app();

    paste(&mut app, "/");

    assert_eq!(
        app.selected_command().as_deref(),
        Some("new"),
        "the peek opens on its first row, as it does when `/` is typed"
    );
}

#[test]
fn an_open_prompt_overlay_takes_the_paste_flattened_to_one_line() {
    let mut app = App::new(Arc::new(MockRuntime::empty()), loaded());
    app.tab_index = tab_index("Hosts");
    app.focus_routing_subpage("Add Host");
    // Local leads the kind picker, so a remote add is Down then two confirms:
    // the second opens the address prompt, which is the single-line field here.
    let _ = app.on_event(key(KeyCode::Down));
    let _ = app.on_event(key(KeyCode::Char('a')));
    let _ = app.on_event(key(KeyCode::Char('a')));
    assert!(app.prompt_state().is_some(), "the address prompt is open");

    let cmd = paste(&mut app, "host:1234\r\nmy label\n");

    assert!(cmd.is_none(), "a paste never submits the prompt either");
    assert_eq!(
        app.prompt_state().map(|(_, text)| text),
        Some("host:1234 my label ".into()),
        "a single-line field has no second row to put a break on"
    );
}

#[test]
fn a_paste_outside_a_text_field_is_ignored() {
    let mut app = App::new(Arc::new(MockRuntime::empty()), loaded());
    app.tab_index = tab_index("Settings");

    assert!(paste(&mut app, "stray text").is_none());
    assert_eq!(
        app.draft_text(),
        "",
        "the Agents composer is not silently filled from another tab"
    );
}

/// Paste with a harness attached, which is a second keyboard target: the pane
/// *is* the operator's terminal then, so the payload has to reach the child
/// rather than the composer sitting invisible behind it.
///
/// Unix-only: it runs `/bin/sh` on a real pty, which Windows has no equivalent
/// of.
#[cfg(unix)]
mod attached {
    use super::*;

    use std::collections::HashMap;
    use std::time::{Duration, Instant};

    use ratatui::backend::TestBackend;
    use ratatui::Terminal;

    use medulla::tinyplace::HarnessProvider;
    use medulla_tui::ui::harness_pane::LocalHarnesses;
    use medulla_tui::worker::pty::{HarnessControl, LaunchSpec, PtyManager};

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
                control: HarnessControl::User,
                user_spawned: true,
            })
            .expect("the shell opens on a pty")
    }

    /// An Agents-tab app attached to `id`, reached the way the operator does:
    /// move the cursor onto the harness row, then press the focus chord.
    fn attached_app(sessions: PtyManager, id: &str) -> App {
        let mut app = agents_app();
        app.set_local_harnesses(LocalHarnesses {
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
            if app.harness_pane_session_for_test().is_some() {
                break;
            }
            let _ = app.on_event(Event::Key(KeyEvent::new(KeyCode::Down, KeyModifiers::ALT)));
        }
        draw(&mut app);
        let _ = app.on_event(Event::Key(KeyEvent::new(
            KeyCode::Char(']'),
            KeyModifiers::CONTROL,
        )));
        assert_eq!(app.attached_harness(), Some(id), "{}", app.status());
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
            app.attached_harness(),
            Some(id.as_str()),
            "the question is asked with the pane still attached"
        );

        assert!(paste(&mut app, "stray text\n").is_none());

        // Answering keeps the harness but returns the keyboard; the shell is
        // still waiting on its `read`, which is how we know nothing was typed
        // into it, and the composer is untouched.
        let _ = app.on_event(key(KeyCode::Char('n')));
        assert_eq!(app.attached_harness(), None, "{}", app.status());
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
}
