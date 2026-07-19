//! Unit tests for the login screen: provider cycling, the loopback/token
//! phases, outcome transitions, inline error rendering, and the token-display
//! helper.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::backend::TestBackend;
use ratatui::Terminal;

use medulla::auth::Provider;

use super::draw::token_display;
use super::{LoginCmd, LoginEvent, LoginOutcome, LoginScreen};

fn key(code: KeyCode) -> KeyEvent {
    KeyEvent::new(code, KeyModifiers::NONE)
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
    assert!(out.contains("google"), "default provider: {out}");
}

#[test]
fn provider_cycles_with_arrows_and_p() {
    let mut s = LoginScreen::new("x");
    assert_eq!(s.provider(), Provider::Google);
    assert!(s.handle_key(key(KeyCode::Right)).is_none());
    assert_eq!(s.provider(), Provider::Github);
    s.handle_key(key(KeyCode::Char('p')));
    assert_eq!(s.provider(), Provider::Twitter);
    s.handle_key(key(KeyCode::Left));
    assert_eq!(s.provider(), Provider::Github);
    s.handle_key(key(KeyCode::Left));
    assert_eq!(s.provider(), Provider::Google);
    // Wrap backwards past the start.
    s.handle_key(key(KeyCode::Left));
    assert_eq!(s.provider(), Provider::Discord);
}

#[test]
fn enter_and_o_start_loopback() {
    let mut s = LoginScreen::new("http://b");
    let cmd = s.handle_key(key(KeyCode::Enter));
    assert_eq!(
        cmd,
        Some(LoginCmd::StartLoopback {
            base_url: "http://b".into(),
            provider: Provider::Google,
        })
    );
    // 'o' also starts (from Idle after a cancel).
    s.apply(LoginEvent::CallbackError("cancelled".into()));
    let cmd = s.handle_key(key(KeyCode::Char('o')));
    assert!(matches!(cmd, Some(LoginCmd::StartLoopback { .. })));
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
    assert!(s.handle_key(key(KeyCode::Char('t'))).is_none());
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
    s2.handle_key(key(KeyCode::Char('t')));
    assert!(s2.handle_key(key(KeyCode::Enter)).is_none());
    assert!(render(&mut s2).contains("enter a token"));
}

#[test]
fn m_and_q_yield_outcomes() {
    let mut m = LoginScreen::new("b");
    m.handle_key(key(KeyCode::Char('m')));
    assert_eq!(m.outcome(), Some(LoginOutcome::Mock));

    let mut q = LoginScreen::new("b");
    q.handle_key(key(KeyCode::Char('q')));
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
    assert!(out.contains("retry"), "retry hint: {out}");
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
