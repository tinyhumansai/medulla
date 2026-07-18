//! Feature tests for the pre-app login screen ([`medulla::ui::login`]): pure
//! rendering and key/event transitions, driven entirely through the public
//! `LoginScreen` API (no async, no real browser, no network).

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::backend::TestBackend;
use ratatui::Terminal;

use medulla::auth::Provider;
use medulla::ui::login::{LoginCmd, LoginEvent, LoginOutcome, LoginScreen};

fn key(code: KeyCode) -> KeyEvent {
    KeyEvent::new(code, KeyModifiers::NONE)
}

/// Render the screen into an 80x24 test terminal and flatten the buffer to text.
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
fn renders_branding_backend_and_provider() {
    let mut s = LoginScreen::new("http://localhost:5000");
    let out = render(&mut s);
    assert!(out.contains("MEDULLA"), "branding: {out}");
    assert!(out.contains("http://localhost:5000"), "backend url: {out}");
    assert!(out.contains("google"), "default provider shown: {out}");
    // Idle menu hints are visible.
    assert!(out.contains("mock"), "mock hint: {out}");
    assert!(out.contains("quit"), "quit hint: {out}");
}

#[test]
fn provider_cycles_and_renders_selection() {
    let mut s = LoginScreen::new("b");
    s.handle_key(key(KeyCode::Right));
    assert_eq!(s.provider(), Provider::Github);
    assert!(render(&mut s).contains("github"));
    s.handle_key(key(KeyCode::Char('p')));
    assert_eq!(s.provider(), Provider::Twitter);
    s.handle_key(key(KeyCode::Left));
    assert_eq!(s.provider(), Provider::Github);
}

#[test]
fn enter_starts_loopback_and_waiting_shows_url_and_port() {
    let mut s = LoginScreen::new("http://backend");
    let cmd = s.handle_key(key(KeyCode::Enter));
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
fn o_also_starts_loopback() {
    let mut s = LoginScreen::new("b");
    assert!(matches!(
        s.handle_key(key(KeyCode::Char('o'))),
        Some(LoginCmd::StartLoopback { .. })
    ));
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
    assert!(render(&mut s).contains("continue offline"));
}

#[test]
fn token_input_mode_edits_and_submits() {
    let mut s = LoginScreen::new("b");
    assert!(s.handle_key(key(KeyCode::Char('t'))).is_none());
    assert!(render(&mut s).contains("Paste a JWT"), "input prompt shown");
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
    s.handle_key(key(KeyCode::Char('t')));
    s.handle_key(key(KeyCode::Char('x')));
    assert!(s.handle_key(key(KeyCode::Esc)).is_none());
    assert!(render(&mut s).contains("continue offline"));
}

#[test]
fn m_yields_mock_outcome() {
    let mut s = LoginScreen::new("b");
    assert!(s.handle_key(key(KeyCode::Char('m'))).is_none());
    assert_eq!(s.outcome(), Some(LoginOutcome::Mock));
}

#[test]
fn q_and_ctrl_c_yield_quit_outcome() {
    let mut s = LoginScreen::new("b");
    s.handle_key(key(KeyCode::Char('q')));
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
    assert!(out.contains("retry"), "retry hint: {out}");
    // Screen stays usable — can start again.
    assert!(matches!(
        s.handle_key(key(KeyCode::Enter)),
        Some(LoginCmd::StartLoopback { .. })
    ));

    let mut s2 = LoginScreen::new("b");
    s2.apply(LoginEvent::VerifyFailed("verification failed: nope".into()));
    assert!(render(&mut s2).contains("verification failed: nope"));
}
