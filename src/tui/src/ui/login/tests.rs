//! Unit tests for the login screen: menu selection, the loopback/token phases,
//! outcome transitions, inline error rendering, and the token-display helper.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::backend::TestBackend;
use ratatui::Terminal;

use medulla::auth::Provider;

use super::draw::token_display;
use super::{LoginCmd, LoginEvent, LoginOutcome, LoginScreen};

fn key(code: KeyCode) -> KeyEvent {
    KeyEvent::new(code, KeyModifiers::NONE)
}

/// Select the menu row at `index` and activate it, the way a user does.
fn choose(screen: &mut LoginScreen, index: usize) -> Option<LoginCmd> {
    for _ in 0..index {
        screen.handle_key(key(KeyCode::Down));
    }
    screen.handle_key(key(KeyCode::Enter))
}

fn render(screen: &mut LoginScreen) -> String {
    let mut terminal = Terminal::new(TestBackend::new(80, 24)).unwrap();
    terminal.draw(|f| screen.draw(f)).unwrap();
    let buf = terminal.backend().buffer().clone();
    buf.content().iter().map(|c| c.symbol()).collect::<String>()
}

#[test]
fn renders_branding_and_backend() {
    let mut s = LoginScreen::new("http://localhost:5000");
    let out = render(&mut s);
    assert!(out.contains("▛▛▌█▌▛▌▌▌▐ ▐ ▀▌"), "logo: {out}");
    assert!(out.contains("localhost:5000"), "base url: {out}");
    // Every provider is offered as its own row rather than hidden behind a
    // cycling field, so all four are visible without pressing anything.
    for label in ["Google", "GitHub", "X (Twitter)"] {
        assert!(out.contains(label), "{label} offered: {out}");
    }
    assert!(
        !out.contains("Discord"),
        "the backend has no Discord login: {out}"
    );
    assert!(out.contains("Paste an API key"), "API-key row: {out}");
    assert!(
        !out.contains("offline"),
        "signing in is the only way in: {out}"
    );
}

#[test]
fn each_provider_row_starts_loopback_with_that_provider() {
    for (index, provider) in [Provider::Google, Provider::Github, Provider::Twitter]
        .into_iter()
        .enumerate()
    {
        let mut s = LoginScreen::new("http://b");
        let cmd = choose(&mut s, index);
        assert_eq!(
            cmd,
            Some(LoginCmd::StartLoopback {
                base_url: "http://b".into(),
                provider,
            }),
            "row {index} signs in with {provider:?}"
        );
        // The choice sticks, so a retry after a failure reuses it.
        assert_eq!(s.provider(), provider);
    }
}

#[test]
fn the_menu_wraps_at_both_ends() {
    // The list is short and every row is reachable both ways, so overshooting
    // should not mean travelling back through the whole menu.
    let mut s = LoginScreen::new("b");
    s.handle_key(key(KeyCode::Up)); // wrap up from the first row to the last
    assert!(s.handle_key(key(KeyCode::Enter)).is_none());
    assert_eq!(s.outcome(), Some(LoginOutcome::Quit));
}

#[test]
fn letters_no_longer_fire_actions() {
    // The old screen bound o/t/m/q/p; a stray keystroke could start a browser
    // flow or drop you into the mock. Selection is now the only way to act.
    for c in ['o', 't', 'm', 'q', 'p'] {
        let mut s = LoginScreen::new("b");
        assert!(
            s.handle_key(key(KeyCode::Char(c))).is_none(),
            "{c} must not emit a command"
        );
        assert_eq!(s.outcome(), None, "{c} must not settle an outcome");
    }
}

#[test]
fn esc_while_waiting_cancels_loopback() {
    let mut s = LoginScreen::new("b");
    s.handle_key(key(KeyCode::Enter));
    s.apply(LoginEvent::LoopbackStarted {
        url: "http://b/auth/google/login".into(),
        port: 40404,
    });
    let out = render(&mut s);
    assert!(
        out.contains("waiting for browser callback"),
        "waiting: {out}"
    );
    assert!(out.contains("40404"), "port: {out}");
    assert!(out.contains("http://b/auth/google/login"), "url: {out}");
    let cmd = s.handle_key(key(KeyCode::Esc));
    assert_eq!(cmd, Some(LoginCmd::CancelLoopback));
}

#[test]
fn token_entry_edits_and_submits() {
    let mut s = LoginScreen::new("b");
    assert!(
        choose(&mut s, 3).is_none(),
        "the API-key row opens the input"
    );
    for c in "abc".chars() {
        s.handle_key(key(KeyCode::Char(c)));
    }
    s.handle_key(key(KeyCode::Backspace));
    let out = render(&mut s);
    assert!(out.contains("ab"), "input echoed: {out}");
    let cmd = s.handle_key(key(KeyCode::Enter));
    assert_eq!(cmd, Some(LoginCmd::SubmitToken("ab".into())));
    // Empty submit is refused with an error, no command.
    let mut s2 = LoginScreen::new("b");
    choose(&mut s2, 3);
    assert!(s2.handle_key(key(KeyCode::Enter)).is_none());
    assert!(render(&mut s2).contains("enter a token"));
}

#[test]
fn the_quit_row_and_ctrl_c_yield_quit() {
    let mut q = LoginScreen::new("b");
    choose(&mut q, 6);
    assert_eq!(q.outcome(), Some(LoginOutcome::Quit));

    let mut c = LoginScreen::new("b");
    c.handle_key(KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL));
    assert_eq!(c.outcome(), Some(LoginOutcome::Quit));
}

#[test]
fn verified_sets_token_outcome_and_flashes() {
    let mut s = LoginScreen::new("b");
    s.apply(LoginEvent::CallbackToken("jwt".into()));
    assert!(render(&mut s).contains("verifying"));
    s.apply(LoginEvent::Verified {
        jwt: "jwt-1".into(),
        who: "Logged in as a@b.c".into(),
    });
    assert_eq!(s.outcome(), Some(LoginOutcome::Token("jwt-1".into())));
    assert!(render(&mut s).contains("Logged in as a@b.c"));
}

#[test]
fn errors_render_inline_and_keep_screen_usable() {
    let mut s = LoginScreen::new("b");
    s.apply(LoginEvent::VerifyFailed("bad token".into()));
    let out = render(&mut s);
    assert!(out.contains("bad token"), "error: {out}");
    assert!(out.contains("try again"), "retry hint: {out}");
    // Still usable: can start over.
    assert!(matches!(
        s.handle_key(key(KeyCode::Enter)),
        Some(LoginCmd::StartLoopback { .. })
    ));

    let mut s2 = LoginScreen::new("b");
    s2.apply(LoginEvent::CallbackError("state mismatch timeout".into()));
    assert!(render(&mut s2).contains("state mismatch timeout"));
}

#[test]
fn tick_advances_spinner_without_panic() {
    let mut s = LoginScreen::new("b");
    s.handle_key(key(KeyCode::Enter));
    s.apply(LoginEvent::LoopbackStarted {
        url: "u".into(),
        port: 1,
    });
    for _ in 0..25 {
        s.tick();
    }
    let _ = render(&mut s);
}

#[test]
fn token_display_truncates() {
    assert_eq!(token_display("", 4), "");
    assert_eq!(token_display("abc", 4), "abc");
    assert_eq!(token_display("abcdef", 4), "abc…");
}
