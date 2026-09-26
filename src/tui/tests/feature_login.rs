//! Feature tests for the pre-app login screen ([`medulla_tui::ui::login`]): pure
//! rendering and key/event transitions, driven entirely through the public
//! `LoginScreen` API (no async, no real browser, no network).

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::backend::TestBackend;
use ratatui::Terminal;

use medulla::auth::Provider;
use medulla_tui::ui::login::{LoginCmd, LoginEvent, LoginOutcome, LoginScreen, SignInMethod};

/// Idle-menu row indices, mirroring the screen's own menu order.
const ROW_BROWSER: usize = 0;
const ROW_CODE: usize = 1;
const ROW_PASTE_KEY: usize = 2;
const ROW_DOCS: usize = 3;
const ROW_STAR: usize = 4;
const ROW_QUIT: usize = 5;

fn key(code: KeyCode) -> KeyEvent {
    KeyEvent::new(code, KeyModifiers::NONE)
}

/// Select the list row at `index` and activate it, the way a user does.
fn choose(screen: &mut LoginScreen, index: usize) -> Option<LoginCmd> {
    for _ in 0..index {
        screen.handle_key(key(KeyCode::Down));
    }
    screen.handle_key(key(KeyCode::Enter))
}

/// Walk the whole sign-in path: pick a method, then a provider.
fn sign_in(screen: &mut LoginScreen, method_row: usize, provider_row: usize) -> Option<LoginCmd> {
    choose(screen, method_row);
    choose(screen, provider_row)
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
fn renders_branding_backend_and_both_sign_in_methods() {
    let mut s = LoginScreen::new("http://localhost:5000");
    let out = render(&mut s);
    assert!(out.contains("▛▛▌█▌▛▌▌▌▐ ▐ ▀▌"), "logo: {out}");
    assert!(out.contains("http://localhost:5000"), "backend url: {out}");
    // The method comes first because it is the choice that decides whether
    // signing in can work here at all: loopback needs a browser on this machine.
    assert!(out.contains("Sign in with a browser"), "browser row: {out}");
    assert!(
        out.contains("SSH"),
        "the code row names its audience: {out}"
    );
    assert!(out.contains("Paste an API key"), "API-key row: {out}");
    assert!(out.contains("↑↓ choose"), "selection hint: {out}");
}

#[test]
fn the_provider_step_lists_every_supported_provider() {
    let mut s = LoginScreen::new("b");
    choose(&mut s, ROW_BROWSER);
    let out = render(&mut s);
    // Each provider is its own row, so the options are readable at a glance
    // rather than hidden behind a field you have to cycle.
    for label in ["Google", "GitHub", "X (Twitter)"] {
        assert!(out.contains(label), "{label} offered: {out}");
    }
}

#[test]
fn the_menu_offers_no_discord_and_no_offline_escape() {
    // The backend has no Discord login, and signing in is the only way into the
    // app — neither may be presented as an option, on either step.
    let mut s = LoginScreen::new("b");
    let menu = render(&mut s);
    assert!(!menu.contains("Discord"), "no Discord row: {menu}");
    assert!(!menu.contains("offline"), "no offline row: {menu}");
    assert!(!menu.contains("mock"), "no mock row: {menu}");

    choose(&mut s, ROW_BROWSER);
    let providers = render(&mut s);
    assert!(
        !providers.contains("Discord"),
        "no Discord provider: {providers}"
    );
}

#[test]
fn selecting_a_provider_row_signs_in_with_it() {
    let mut s = LoginScreen::new("b");
    let cmd = sign_in(&mut s, ROW_BROWSER, 1); // GitHub
    assert_eq!(
        cmd,
        Some(LoginCmd::StartLoopback {
            base_url: "b".into(),
            provider: Provider::Github,
        })
    );
    assert_eq!(s.provider(), Provider::Github);
    assert_eq!(s.method(), SignInMethod::Browser);
}

#[test]
fn enter_starts_loopback_and_waiting_shows_url_and_port() {
    let mut s = LoginScreen::new("http://backend");
    let cmd = sign_in(&mut s, ROW_BROWSER, 0);
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
fn the_code_method_shows_a_url_for_another_device_and_binds_nothing() {
    let mut s = LoginScreen::new("http://backend");
    // No command: there is no listener to start. That is the point — over SSH
    // the loopback callback would reach the browser host, not this one.
    assert_eq!(sign_in(&mut s, ROW_CODE, 0), None);
    assert_eq!(s.method(), SignInMethod::Code);

    let out = render(&mut s);
    assert!(
        out.contains("http://backend/auth/google/login?redirect=cli"),
        "verification url: {out}"
    );
    assert!(out.contains("any device"), "open-anywhere hint: {out}");
    assert!(out.contains("Paste the code"), "paste prompt: {out}");
    assert!(
        !out.contains("127.0.0.1"),
        "nothing is bound locally: {out}"
    );
}

#[test]
fn a_pasted_code_is_submitted_for_redemption() {
    let mut s = LoginScreen::new("b");
    sign_in(&mut s, ROW_CODE, 0);
    // A code copied out of the browser usually carries a trailing newline; it
    // must land in the field rather than submitting itself half-typed.
    s.handle_paste(&format!("{}\n", "d".repeat(64)));
    assert_eq!(
        s.handle_key(key(KeyCode::Enter)),
        Some(LoginCmd::SubmitToken("d".repeat(64)))
    );
    assert!(render(&mut s).contains("verifying"));
}

#[test]
fn esc_cancels_waiting() {
    let mut s = LoginScreen::new("b");
    sign_in(&mut s, ROW_BROWSER, 0);
    s.apply(LoginEvent::LoopbackStarted {
        url: "u".into(),
        port: 1,
    });
    assert_eq!(
        s.handle_key(key(KeyCode::Esc)),
        Some(LoginCmd::CancelLoopback)
    );
    // Back to the provider list after cancel, so another provider is one key away.
    assert!(render(&mut s).contains("choose a provider"));
}

#[test]
fn token_input_mode_edits_and_submits() {
    let mut s = LoginScreen::new("b");
    assert!(
        choose(&mut s, ROW_PASTE_KEY).is_none(),
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
    choose(&mut s, ROW_PASTE_KEY);
    s.handle_key(key(KeyCode::Char('x')));
    assert!(s.handle_key(key(KeyCode::Esc)).is_none());
    assert!(render(&mut s).contains("Sign in with a browser"));
}

#[test]
fn the_quit_row_and_ctrl_c_yield_quit_outcome() {
    let mut s = LoginScreen::new("b");
    choose(&mut s, ROW_QUIT);
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
    assert!(s.handle_key(key(KeyCode::Enter)).is_none());
    assert!(render(&mut s).contains("choose a provider"));

    let mut s2 = LoginScreen::new("b");
    s2.apply(LoginEvent::VerifyFailed("verification failed: nope".into()));
    assert!(render(&mut s2).contains("verification failed: nope"));
}

#[test]
fn the_docs_and_github_rows_open_links_without_disturbing_the_menu() {
    // Reading the docs is not an answer to "how do I sign in", so opening one
    // must leave the screen exactly where it was — same phase, no outcome.
    for (index, expected) in [
        (ROW_DOCS, "https://tinyhumans.gitbook.io/medulla"),
        (ROW_STAR, "https://github.com/tinyhumansai/medulla"),
    ] {
        let mut s = LoginScreen::new("b");
        let cmd = choose(&mut s, index);
        assert_eq!(cmd, Some(LoginCmd::OpenUrl(expected.into())), "row {index}");
        assert_eq!(s.outcome(), None, "opening a link must not end the screen");
        // Still on the menu and still able to sign in.
        let out = render(&mut s);
        assert!(out.contains("Sign in with a browser"), "menu intact: {out}");
    }
}

#[test]
fn the_menu_lists_the_docs_and_github_rows() {
    let mut s = LoginScreen::new("b");
    let out = render(&mut s);
    assert!(out.contains("Read the docs"), "docs row: {out}");
    assert!(out.contains("Star us on GitHub"), "github row: {out}");
}
