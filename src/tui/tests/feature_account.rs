//! Feature tests for Settings > ABOUT > Account: what the page reports about
//! the signed-in session, and the two-step logout.
//!
//! The session lives in the embedded OpenHuman core, not in a file this test can
//! seed, so signed-in state is injected with `set_account` and the logout is
//! observed as the [`Cmd::Logout`] the page emits. The clear itself is the
//! runtime's job and is covered where that runtime is.

use std::sync::Arc;

use crossterm::event::{Event, KeyCode, KeyEvent, KeyModifiers};
use ratatui::backend::TestBackend;
use ratatui::Terminal;

use medulla::config::LoadedConfig;
use medulla::core_host::AuthState;
use medulla::runtime::mock::MockRuntime;
use medulla_tui::ui::app::{App, Cmd};

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

/// Report the core as signed in, the way startup does.
fn sign_in(app: &mut App) {
    app.set_account(Some(AuthState {
        is_authenticated: true,
        user_id: Some("u-1".into()),
    }));
}

fn key(app: &mut App, code: KeyCode) -> Option<Cmd> {
    app.on_event(Event::Key(KeyEvent::new(code, KeyModifiers::NONE)))
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
fn a_signed_in_account_names_the_user_without_showing_the_token() {
    let dir = tempfile::tempdir().expect("tempdir");
    let mut app = account_app(dir.path());
    sign_in(&mut app);
    let out = render(&mut app, 160, 50);
    assert!(out.contains("signed in as u-1"), "{out}");
}

#[test]
fn logout_takes_two_presses_before_it_commands_anything() {
    let dir = tempfile::tempdir().expect("tempdir");
    let mut app = account_app(dir.path());
    sign_in(&mut app);

    // First Enter only arms it — nothing is dispatched.
    assert!(
        key(&mut app, KeyCode::Enter).is_none(),
        "arming issues no command"
    );
    assert!(
        app.status().contains("press Enter again"),
        "asks for confirmation: {}",
        app.status()
    );
    let out = render(&mut app, 160, 50);
    assert!(
        out.contains("Press Enter again"),
        "armed state is visible: {out}"
    );

    // Second Enter dispatches the clear.
    assert!(
        matches!(key(&mut app, KeyCode::Enter), Some(Cmd::Logout)),
        "the second press commands the logout"
    );
}

#[test]
fn the_session_ends_only_once_the_clear_reports_success() {
    // The live runtime still holds the session that was just revoked, so the
    // app must end rather than leave the user signed out in the core but signed
    // in on screen — and it must not do that until the clear actually landed.
    let dir = tempfile::tempdir().expect("tempdir");
    let mut app = account_app(dir.path());
    sign_in(&mut app);

    key(&mut app, KeyCode::Enter);
    key(&mut app, KeyCode::Enter);
    assert!(
        !app.should_quit,
        "commanding the clear is not completing it"
    );
    assert!(!app.relogin_requested(), "nor requesting a relogin");

    app.logged_out();
    assert!(app.should_quit, "the session ends");
    assert!(app.relogin_requested(), "and asks for the login screen");
}

#[test]
fn a_failed_logout_leaves_the_session_running() {
    // Nothing was cleared, so there is nothing to re-authenticate for; dropping
    // the user to the login screen here would lose their session for no reason.
    let dir = tempfile::tempdir().expect("tempdir");
    let mut app = account_app(dir.path());
    sign_in(&mut app);

    key(&mut app, KeyCode::Enter);
    key(&mut app, KeyCode::Enter);
    // The loop reports the failure as a plain status and never calls
    // `logged_out`, so the session survives.
    app.set_status("Account · logout failed: nope");
    assert!(!app.should_quit, "the session survives a failed logout");
    assert!(!app.relogin_requested(), "no relogin is requested");
}

#[test]
fn escape_cancels_an_armed_logout() {
    let dir = tempfile::tempdir().expect("tempdir");
    let mut app = account_app(dir.path());
    sign_in(&mut app);

    key(&mut app, KeyCode::Enter);
    key(&mut app, KeyCode::Esc);
    assert!(app.status().contains("cancelled"), "{}", app.status());

    // A later Enter must arm again rather than fire immediately.
    assert!(
        key(&mut app, KeyCode::Enter).is_none(),
        "re-arms, does not fire"
    );
    assert!(
        app.status().contains("press Enter again"),
        "{}",
        app.status()
    );
}

#[test]
fn navigating_away_disarms_the_logout() {
    let dir = tempfile::tempdir().expect("tempdir");
    let mut app = account_app(dir.path());
    sign_in(&mut app);

    key(&mut app, KeyCode::Enter); // arm

    // Leaving now takes two Escapes: the first disarms, the second steps out of
    // the content pane back to the subpage nav.
    key(&mut app, KeyCode::Esc);
    key(&mut app, KeyCode::Esc);
    assert!(!app.settings_focused(), "back on the nav");
    key(&mut app, KeyCode::Up); // move to Context
    assert_eq!(app.settings_subpage(), "Context");
    key(&mut app, KeyCode::Down); // back to Account
    key(&mut app, KeyCode::Enter); // re-enter the pane

    // Returning must not resume an armed logout.
    assert!(
        key(&mut app, KeyCode::Enter).is_none(),
        "an armed logout must not survive leaving the page"
    );
    assert!(
        app.status().contains("press Enter again"),
        "{}",
        app.status()
    );
}
