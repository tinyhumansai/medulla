//! Unit tests for the onboarding screen's step machine and rendering.

use super::*;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::backend::TestBackend;
use ratatui::Terminal;

fn key(code: KeyCode) -> KeyEvent {
    KeyEvent::new(code, KeyModifiers::NONE)
}

fn render(screen: &mut OnboardingScreen) -> String {
    let mut terminal = Terminal::new(TestBackend::new(90, 26)).unwrap();
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
fn prefills_name_and_walks_to_identity() {
    let mut s = OnboardingScreen::new("ada@box/10.0.0.4", None, "https://api.tiny.place");
    let out = render(&mut s);
    assert!(out.contains("MEDULLA WORKER"), "branding: {out}");
    assert!(out.contains("ada@box/10.0.0.4"), "prefilled name: {out}");
    assert!(out.contains("Step 1/3"), "step label: {out}");

    let cmd = s.handle_key(key(KeyCode::Enter));
    assert_eq!(
        cmd,
        Some(OnboardingCmd::LoadIdentity {
            name: "ada@box/10.0.0.4".into()
        })
    );
    assert!(render(&mut s).contains("setting up the tiny.place identity"));
}

#[test]
fn name_is_editable() {
    let mut s = OnboardingScreen::new("abc", None, "e");
    s.handle_key(key(KeyCode::Backspace));
    s.handle_key(key(KeyCode::Char('x')));
    assert_eq!(s.name(), "abx");
    assert!(render(&mut s).contains("abx"));
}

#[test]
fn empty_name_is_refused() {
    let mut s = OnboardingScreen::new("", None, "e");
    assert!(s.handle_key(key(KeyCode::Enter)).is_none());
    assert!(render(&mut s).contains("enter a worker name"));
}

#[test]
fn identity_ready_advances_and_shows_address_and_handle() {
    let mut s = OnboardingScreen::new("w", None, "e");
    s.handle_key(key(KeyCode::Enter));
    s.apply(OnboardingEvent::IdentityReady {
        address: "AgentAddr111".into(),
        handle: Some("@ada".into()),
    });
    let out = render(&mut s);
    assert!(out.contains("AgentAddr111"), "address: {out}");
    assert!(out.contains("@ada"), "handle: {out}");
    assert!(out.contains("OpenHuman owner"), "owner prompt: {out}");
}

#[test]
fn env_owner_prefills_owner_field() {
    let mut s = OnboardingScreen::new("w", Some("@overseer".into()), "e");
    s.handle_key(key(KeyCode::Enter));
    s.apply(OnboardingEvent::IdentityReady {
        address: "A".into(),
        handle: None,
    });
    assert!(render(&mut s).contains("@overseer"), "env owner prefilled");
}

#[test]
fn owner_entered_then_confirmed_registers() {
    let mut s = OnboardingScreen::new("w", None, "e");
    s.handle_key(key(KeyCode::Enter));
    s.apply(OnboardingEvent::IdentityReady {
        address: "A".into(),
        handle: None,
    });
    for c in "@boss".chars() {
        s.handle_key(key(KeyCode::Char(c)));
    }
    // Enter → confirm step.
    assert!(s.handle_key(key(KeyCode::Enter)).is_none());
    let out = render(&mut s);
    assert!(out.contains("Step 3/3"), "confirm step: {out}");
    assert!(out.contains("@boss"), "owner in summary: {out}");
    // Enter → register.
    s.handle_key(key(KeyCode::Enter));
    assert_eq!(
        s.outcome(),
        Some(OnboardingOutcome::Register {
            name: "w".into(),
            owner: Some("@boss".into())
        })
    );
}

#[test]
fn esc_skips_owner_with_a_note() {
    let mut s = OnboardingScreen::new("w", Some("@pre".into()), "e");
    s.handle_key(key(KeyCode::Enter));
    s.apply(OnboardingEvent::IdentityReady {
        address: "A".into(),
        handle: None,
    });
    // Esc skips — forgetting the prefilled owner.
    s.handle_key(key(KeyCode::Esc));
    let out = render(&mut s);
    assert!(out.contains("set later"), "skip note: {out}");
    assert!(out.contains("(none"), "owner none in summary: {out}");
    s.handle_key(key(KeyCode::Enter));
    assert_eq!(
        s.outcome(),
        Some(OnboardingOutcome::Register {
            name: "w".into(),
            owner: None
        })
    );
}

#[test]
fn confirm_esc_returns_to_owner_editing() {
    let mut s = OnboardingScreen::new("w", None, "e");
    s.handle_key(key(KeyCode::Enter));
    s.apply(OnboardingEvent::IdentityReady {
        address: "A".into(),
        handle: None,
    });
    s.handle_key(key(KeyCode::Enter)); // → confirm
    s.handle_key(key(KeyCode::Esc)); // → back to owner
    assert!(render(&mut s).contains("OpenHuman owner"));
}

#[test]
fn q_and_ctrl_c_abort() {
    let mut s = OnboardingScreen::new("w", None, "e");
    s.handle_key(key(KeyCode::Enter));
    s.apply(OnboardingEvent::IdentityReady {
        address: "A".into(),
        handle: None,
    });
    s.handle_key(key(KeyCode::Enter)); // → confirm
    s.handle_key(key(KeyCode::Char('q')));
    assert_eq!(s.outcome(), Some(OnboardingOutcome::Abort));

    let mut c = OnboardingScreen::new("w", None, "e");
    c.handle_key(KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL));
    assert_eq!(c.outcome(), Some(OnboardingOutcome::Abort));
}

#[test]
fn identity_failed_returns_to_name_with_error() {
    let mut s = OnboardingScreen::new("w", None, "e");
    s.handle_key(key(KeyCode::Enter));
    s.apply(OnboardingEvent::IdentityFailed("keygen boom".into()));
    let out = render(&mut s);
    assert!(out.contains("keygen boom"), "error shown: {out}");
    assert!(out.contains("Step 1/3"), "back to name: {out}");
    // Still usable: re-submit.
    assert!(matches!(
        s.handle_key(key(KeyCode::Enter)),
        Some(OnboardingCmd::LoadIdentity { .. })
    ));
}

#[test]
fn tick_advances_spinner_without_panic() {
    let mut s = OnboardingScreen::new("w", None, "e");
    s.handle_key(key(KeyCode::Enter));
    for _ in 0..20 {
        s.tick();
    }
    let _ = render(&mut s);
}
