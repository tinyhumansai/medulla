//! Feature tests for the first-run worker onboarding screen
//! ([`medulla_tui::ui::onboarding`]) and the profile model
//! ([`medulla::worker_profile`]): pure rendering and key/event transitions plus
//! profile round-tripping, driven entirely through the public API (no async, no
//! TTY, no network).

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::backend::TestBackend;
use ratatui::Terminal;

use medulla::worker_profile::{
    compose_worker_name, default_worker_name, is_registered, WorkerProfile,
};
use medulla_tui::ui::onboarding::{
    OnboardingCmd, OnboardingEvent, OnboardingOutcome, OnboardingScreen,
};

fn key(code: KeyCode) -> KeyEvent {
    KeyEvent::new(code, KeyModifiers::NONE)
}

/// Render the screen into a test terminal and flatten the buffer to text.
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
fn renders_step_one_with_prefilled_name() {
    let mut s = OnboardingScreen::new("ada@box/10.0.0.4", None, "https://api.tiny.place");
    let out = render(&mut s);
    assert!(out.contains("MEDULLA WORKER"), "branding: {out}");
    assert!(out.contains("first-run registration"), "subtitle: {out}");
    assert!(out.contains("Step 1/3"), "step one: {out}");
    assert!(out.contains("ada@box/10.0.0.4"), "prefilled name: {out}");
}

#[test]
fn full_happy_path_registers_with_owner() {
    let mut s = OnboardingScreen::new("worker-1", Some("@lead".into()), "https://api.tiny.place");
    // Step 1: accept the name → LoadIdentity.
    let cmd = s.handle_key(key(KeyCode::Enter));
    assert_eq!(
        cmd,
        Some(OnboardingCmd::LoadIdentity {
            name: "worker-1".into()
        })
    );
    assert!(render(&mut s).contains("setting up the tiny.place identity"));

    // Identity resolves → owner step, address + prefilled env owner shown.
    s.apply(OnboardingEvent::IdentityReady {
        address: "Addr9xQ".into(),
        handle: Some("@worker".into()),
    });
    let out = render(&mut s);
    assert!(out.contains("Step 2/3"), "connection step: {out}");
    assert!(out.contains("Addr9xQ"), "address: {out}");
    assert!(out.contains("@worker"), "handle: {out}");
    assert!(out.contains("@lead"), "env owner prefilled: {out}");

    // Accept the owner → confirm summary.
    s.handle_key(key(KeyCode::Enter));
    let out = render(&mut s);
    assert!(out.contains("Step 3/3"), "confirm step: {out}");
    assert!(out.contains("worker-1"), "name in summary: {out}");
    assert!(out.contains("@lead"), "owner in summary: {out}");
    assert!(out.contains("api.tiny.place"), "endpoint in summary: {out}");

    // Finish.
    s.handle_key(key(KeyCode::Enter));
    assert_eq!(
        s.outcome(),
        Some(OnboardingOutcome::Register {
            name: "worker-1".into(),
            owner: Some("@lead".into()),
        })
    );
}

#[test]
fn owner_can_be_skipped_with_esc() {
    let mut s = OnboardingScreen::new("w", Some("@prefill".into()), "e");
    s.handle_key(key(KeyCode::Enter));
    s.apply(OnboardingEvent::IdentityReady {
        address: "A".into(),
        handle: None,
    });
    // Esc skips the owner → confirm with none.
    s.handle_key(key(KeyCode::Esc));
    assert!(render(&mut s).contains("set later"), "skip note shown");
    s.handle_key(key(KeyCode::Enter));
    assert_eq!(
        s.outcome(),
        Some(OnboardingOutcome::Register {
            name: "w".into(),
            owner: None,
        })
    );
}

#[test]
fn abort_yields_no_registration() {
    let mut s = OnboardingScreen::new("w", None, "e");
    s.handle_key(KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL));
    assert_eq!(s.outcome(), Some(OnboardingOutcome::Abort));
}

#[test]
fn identity_failure_returns_to_name_step() {
    let mut s = OnboardingScreen::new("w", None, "e");
    s.handle_key(key(KeyCode::Enter));
    s.apply(OnboardingEvent::IdentityFailed("network down".into()));
    let out = render(&mut s);
    assert!(out.contains("network down"), "error: {out}");
    assert!(out.contains("Step 1/3"), "back to name: {out}");
}

#[test]
fn default_worker_name_shape_and_registered_detection() {
    let mut env = std::collections::HashMap::new();
    env.insert("USER".to_string(), "ada".to_string());
    env.insert("HOSTNAME".to_string(), "box-9".to_string());
    let name = default_worker_name(&env);
    assert!(name.starts_with("ada@box-9/"), "name shape: {name}");
    assert_eq!(compose_worker_name("u", "h", "1.2.3.4"), "u@h/1.2.3.4");

    let profile = WorkerProfile {
        name,
        ..Default::default()
    };
    assert!(is_registered(Some(&profile), true));
    assert!(!is_registered(Some(&profile), false));
    assert!(!is_registered(None, true));
}

#[test]
fn profile_persists_and_reloads() {
    let dir = std::env::temp_dir().join(format!("medulla-feat-onb-{}", std::process::id()));
    let _ = std::fs::create_dir_all(&dir);
    let path = dir.join("worker.json");
    let profile = WorkerProfile {
        name: "grace@node/10.1.1.1".to_string(),
        address: "AgentAddr".to_string(),
        owner: Some("@overseer".to_string()),
        registered_at: Some("2026-07-18T12:00:00Z".to_string()),
    };
    profile.save(&path).unwrap();
    assert_eq!(WorkerProfile::load(&path).as_ref(), Some(&profile));
    let _ = std::fs::remove_dir_all(&dir);
}
