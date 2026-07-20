//! Feature tests for Settings > ABOUT > Account: what the page reports about
//! the signed-in backend, and the two-step logout.
//!
//! Logout clears the on-disk credential store, so every test injects a temp
//! Medulla home via `set_medulla_home` and never touches the real one.

use std::sync::Arc;

use crossterm::event::{Event, KeyCode, KeyEvent, KeyModifiers};
use ratatui::backend::TestBackend;
use ratatui::Terminal;

use medulla::auth::{CredentialStore, Credentials};
use medulla::config::LoadedConfig;
use medulla::runtime::mock::MockRuntime;
use medulla_tui::ui::app::App;

/// An app parked on the Account subpage with `home` as its Medulla home.
fn account_app(home: &std::path::Path) -> App {
    let rt = Arc::new(MockRuntime::demo());
    let mut l = LoadedConfig::defaults("medulla.tui.json".into());
    l.config.backend.base_url = "https://api.tinyhumans.ai".into();
    let mut app = App::new(rt, l);
    app.set_medulla_home(home.to_path_buf());
    let _ = app.focus_settings_subpage("Account");
    app
}

/// Seed a credential store under `home` so the app looks signed in.
fn sign_in(home: &std::path::Path) -> CredentialStore {
    let store = CredentialStore::at_home(home);
    store
        .save(&Credentials {
            base_url: "https://api.tinyhumans.ai".into(),
            jwt: "test-token".into(),
        })
        .expect("seed credentials");
    store
}

fn key(app: &mut App, code: KeyCode) {
    app.on_event(Event::Key(KeyEvent::new(code, KeyModifiers::NONE)));
}

fn render(app: &mut App, w: u16, h: u16) -> String {
    let mut terminal = Terminal::new(TestBackend::new(w, h)).unwrap();
    terminal.draw(|f| app.draw(f)).unwrap();
    terminal
        .backend()
        .buffer()
        .content()
        .iter()
        .map(|c| c.symbol())
        .collect()
}

#[test]
fn the_page_names_the_backend_host() {
    let dir = tempfile::tempdir().expect("tempdir");
    let mut app = account_app(dir.path());
    let out = render(&mut app, 160, 50);
    assert!(
        out.contains("api.tinyhumans.ai"),
        "names the backend: {out}"
    );
}

#[test]
fn a_signed_out_account_says_how_to_sign_in() {
    let dir = tempfile::tempdir().expect("tempdir");
    let mut app = account_app(dir.path());
    let out = render(&mut app, 160, 50);
    assert!(out.contains("signed out"), "{out}");
    assert!(
        out.contains("medulla login"),
        "points at the command: {out}"
    );
}

#[test]
fn a_signed_in_account_reports_it_without_showing_the_token() {
    let dir = tempfile::tempdir().expect("tempdir");
    sign_in(dir.path());
    let mut app = account_app(dir.path());
    let out = render(&mut app, 160, 50);
    assert!(out.contains("signed in"), "{out}");
    assert!(
        !out.contains("test-token"),
        "the token must never be rendered: {out}"
    );
}

#[test]
fn logout_takes_two_presses_and_clears_the_store() {
    let dir = tempfile::tempdir().expect("tempdir");
    let store = sign_in(dir.path());
    let mut app = account_app(dir.path());

    // First Enter only arms it — the credentials must survive.
    key(&mut app, KeyCode::Enter);
    assert!(
        app.status().contains("press Enter again"),
        "asks for confirmation: {}",
        app.status()
    );
    assert!(store.load().is_some(), "not cleared by the first press");
    let out = render(&mut app, 160, 50);
    assert!(
        out.contains("Press Enter again"),
        "armed state is visible: {out}"
    );

    // Second Enter performs it.
    key(&mut app, KeyCode::Enter);
    assert!(store.load().is_none(), "credentials cleared");
    assert!(app.status().contains("logged out"), "{}", app.status());
}

#[test]
fn escape_cancels_an_armed_logout() {
    let dir = tempfile::tempdir().expect("tempdir");
    let store = sign_in(dir.path());
    let mut app = account_app(dir.path());

    key(&mut app, KeyCode::Enter);
    key(&mut app, KeyCode::Esc);
    assert!(app.status().contains("cancelled"), "{}", app.status());

    // A later Enter must arm again rather than fire immediately.
    key(&mut app, KeyCode::Enter);
    assert!(store.load().is_some(), "still signed in");
    assert!(
        app.status().contains("press Enter again"),
        "{}",
        app.status()
    );
}

#[test]
fn navigating_away_disarms_the_logout() {
    let dir = tempfile::tempdir().expect("tempdir");
    let store = sign_in(dir.path());
    let mut app = account_app(dir.path());

    key(&mut app, KeyCode::Enter); // arm
    key(&mut app, KeyCode::Up); // move to Context
    assert_eq!(app.settings_subpage(), "Context");
    key(&mut app, KeyCode::Down); // back to Account

    // Returning must not resume an armed logout.
    key(&mut app, KeyCode::Enter);
    assert!(
        store.load().is_some(),
        "an armed logout must not survive leaving the page"
    );
    assert!(
        app.status().contains("press Enter again"),
        "{}",
        app.status()
    );
}

#[test]
fn logout_without_a_medulla_home_reports_rather_than_guessing() {
    let rt = Arc::new(MockRuntime::demo());
    let mut app = App::new(rt, LoadedConfig::defaults("medulla.tui.json".into()));
    let _ = app.focus_settings_subpage("Account");

    key(&mut app, KeyCode::Enter);
    key(&mut app, KeyCode::Enter);

    assert!(
        app.status().contains("no Medulla home"),
        "says why it cannot act: {}",
        app.status()
    );
}
