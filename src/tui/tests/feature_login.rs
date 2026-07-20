//! Feature tests for the pre-app login screen ([`medulla_tui::ui::login`]): pure
//! rendering and key/event transitions, driven entirely through the public
//! `LoginScreen` API (no async, no real browser, no network).

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::backend::TestBackend;
use ratatui::Terminal;

use medulla::auth::Provider;
use medulla_tui::ui::login::{LoginCmd, LoginEvent, LoginOutcome, LoginScreen};

fn key(code: KeyCode) -> KeyEvent {
    KeyEvent::new(code, KeyModifiers::NONE)
}

/// Render the screen into an 80x24 test terminal and flatten the buffer to text.
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
    terminal
        .backend()
        .buffer()
        .content()
        .iter()
        .map(|c| c.symbol())
        .collect()
}

#[test]
fn renders_branding_backend_and_every_sign_in_option() {
    let mut s = LoginScreen::new("http://localhost:5000");
    let out = render(&mut s);
    assert!(out.contains("▛▛▌█▌▛▌▌▌▐ ▐ ▀▌"), "logo: {out}");
    assert!(out.contains("http://localhost:5000"), "backend url: {out}");
    // Each provider is its own row, so the options are readable at a glance
    // rather than hidden behind a field you have to cycle.
    for label in ["Google", "GitHub", "X (Twitter)"] {
        assert!(out.contains(label), "{label} offered: {out}");
    }
    assert!(out.contains("Paste an API key"), "API-key row: {out}");
    assert!(out.contains("↑↓ choose"), "selection hint: {out}");
}

#[test]
fn the_menu_offers_no_discord_and_no_offline_escape() {
    // The backend has no Discord login, and signing in is the only way into the
    // app — neither may be presented as an option.
    let mut s = LoginScreen::new("b");
    let out = render(&mut s);
    assert!(!out.contains("Discord"), "no Discord row: {out}");
    assert!(!out.contains("offline"), "no offline row: {out}");
    assert!(!out.contains("mock"), "no mock row: {out}");
}

#[test]
fn selecting_a_provider_row_signs_in_with_it() {
    let mut s = LoginScreen::new("b");
    let cmd = choose(&mut s, 1); // GitHub
    assert_eq!(
        cmd,
        Some(LoginCmd::StartLoopback {
            base_url: "b".into(),
            provider: Provider::Github,
        })
    );
    assert_eq!(s.provider(), Provider::Github);
}

#[test]
fn enter_starts_loopback_and_waiting_shows_url_and_port() {
    let mut s = LoginScreen::new("http://backend");
    let cmd = choose(&mut s, 0);
    assert_eq!(
        cmd,
        Some(LoginCmd::StartLoopback {
            base_url: "http://backend".into(),
            provider: Provider::Google,
        })
    );
    s.apply(LoginEvent::LoopbackStarted {
        url: "http://backend/auth/google/login?redirectUri=x".into(),
        port: 51234,
    });
    let out = render(&mut s);
    assert!(
        out.contains("waiting for browser callback"),
        "waiting: {out}"
    );
    assert!(out.contains("127.0.0.1:51234"), "port: {out}");
    assert!(out.contains("/auth/google/login"), "login url: {out}");
    assert!(out.contains("Esc"), "cancel hint: {out}");
}

#[test]
fn esc_cancels_waiting() {
    let mut s = LoginScreen::new("b");
    s.handle_key(key(KeyCode::Enter));
    s.apply(LoginEvent::LoopbackStarted {
        url: "u".into(),
        port: 1,
    });
    assert_eq!(
        s.handle_key(key(KeyCode::Esc)),
        Some(LoginCmd::CancelLoopback)
    );
    // Back to the Idle menu after cancel.
    assert!(render(&mut s).contains("Continue with Google"));
}

#[test]
fn token_input_mode_edits_and_submits() {
    let mut s = LoginScreen::new("b");
    assert!(
        choose(&mut s, 3).is_none(),
        "the API-key row opens the input"
    );
    assert!(
        render(&mut s).contains("Paste an API key"),
        "input prompt shown"
    );
    for c in "jwt.token".chars() {
        s.handle_key(key(KeyCode::Char(c)));
    }
    s.handle_key(key(KeyCode::Backspace));
    assert!(render(&mut s).contains("jwt.toke"), "input echoed");
    let cmd = s.handle_key(key(KeyCode::Enter));
    assert_eq!(cmd, Some(LoginCmd::SubmitToken("jwt.toke".into())));
}

#[test]
fn token_input_esc_returns_to_menu() {
    let mut s = LoginScreen::new("b");
    choose(&mut s, 3);
    s.handle_key(key(KeyCode::Char('x')));
    assert!(s.handle_key(key(KeyCode::Esc)).is_none());
    assert!(render(&mut s).contains("Continue with Google"));
}

#[test]
fn the_quit_row_and_ctrl_c_yield_quit_outcome() {
    let mut s = LoginScreen::new("b");
    choose(&mut s, 6);
    assert_eq!(s.outcome(), Some(LoginOutcome::Quit));

    let mut c = LoginScreen::new("b");
    c.handle_key(KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL));
    assert_eq!(c.outcome(), Some(LoginOutcome::Quit));
}

#[test]
fn apply_callback_token_shows_verifying() {
    let mut s = LoginScreen::new("b");
    s.apply(LoginEvent::CallbackToken("jwt".into()));
    assert!(render(&mut s).contains("verifying"));
    assert!(s.outcome().is_none(), "not resolved until verified");
}

#[test]
fn apply_verified_sets_token_outcome() {
    let mut s = LoginScreen::new("b");
    s.apply(LoginEvent::Verified {
        jwt: "the-jwt".into(),
        who: "Logged in as dev@example.com (u1)".into(),
    });
    assert_eq!(s.outcome(), Some(LoginOutcome::Token("the-jwt".into())));
    assert!(render(&mut s).contains("Logged in as dev@example.com"));
}

#[test]
fn apply_callback_error_and_verify_failed_render_inline() {
    let mut s = LoginScreen::new("b");
    s.apply(LoginEvent::CallbackError("state mismatch timeout".into()));
    let out = render(&mut s);
    assert!(
        out.contains("state mismatch timeout"),
        "callback error: {out}"
    );
    assert!(out.contains("try again"), "retry hint: {out}");
    // Screen stays usable — can start again.
    assert!(matches!(
        s.handle_key(key(KeyCode::Enter)),
        Some(LoginCmd::StartLoopback { .. })
    ));

    let mut s2 = LoginScreen::new("b");
    s2.apply(LoginEvent::VerifyFailed("verification failed: nope".into()));
    assert!(render(&mut s2).contains("verification failed: nope"));
}

#[test]
fn the_docs_and_github_rows_open_links_without_disturbing_the_menu() {
    // Reading the docs is not an answer to "how do I sign in", so opening one
    // must leave the screen exactly where it was — same phase, no outcome.
    for (index, expected) in [
        (4, "https://tinyhumans.gitbook.io/medulla"),
        (5, "https://github.com/tinyhumansai/medulla"),
    ] {
        let mut s = LoginScreen::new("b");
        let cmd = choose(&mut s, index);
        assert_eq!(cmd, Some(LoginCmd::OpenUrl(expected.into())), "row {index}");
        assert_eq!(s.outcome(), None, "opening a link must not end the screen");
        // Still on the menu and still able to sign in.
        let out = render(&mut s);
        assert!(out.contains("Continue with Google"), "menu intact: {out}");
    }
}

#[test]
fn the_menu_lists_the_docs_and_github_rows() {
    let mut s = LoginScreen::new("b");
    let out = render(&mut s);
    assert!(out.contains("Read the docs"), "docs row: {out}");
    assert!(out.contains("Star us on GitHub"), "github row: {out}");
}
