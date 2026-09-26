//! Unit tests for the login screen: the method → provider progression, the
//! loopback / code / API-key phases, outcome transitions, inline error
//! rendering, and the token-display helper.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::backend::TestBackend;
use ratatui::Terminal;

use medulla::auth::Provider;

use super::draw::token_display;
use super::{LoginCmd, LoginEvent, LoginOutcome, LoginScreen, SignInMethod};

/// Idle-menu row indices, mirroring `types::MENU`.
const ROW_BROWSER: usize = 0;
const ROW_CODE: usize = 1;
const ROW_PASTE_KEY: usize = 2;
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

fn render(screen: &mut LoginScreen) -> String {
    let mut terminal = Terminal::new(TestBackend::new(80, 24)).unwrap();
    terminal.draw(|f| screen.draw(f)).unwrap();
    let buf = terminal.backend().buffer().clone();
    buf.content().iter().map(|c| c.symbol()).collect::<String>()
}

#[test]
fn renders_branding_and_both_sign_in_methods() {
    let mut s = LoginScreen::new("http://localhost:5000");
    let out = render(&mut s);
    assert!(out.contains("▛▛▌█▌▛▌▌▌▐ ▐ ▀▌"), "logo: {out}");
    assert!(out.contains("localhost:5000"), "base url: {out}");
    // The method is the first choice, because it is the one that decides
    // whether signing in can work at all on this machine.
    assert!(out.contains("Sign in with a browser"), "browser row: {out}");
    assert!(
        out.contains("SSH"),
        "the code row says who it is for: {out}"
    );
    assert!(out.contains("Paste an API key"), "API-key row: {out}");
    assert!(
        !out.contains("offline"),
        "signing in is the only way in: {out}"
    );
}

#[test]
fn the_provider_step_offers_every_supported_provider() {
    let mut s = LoginScreen::new("b");
    choose(&mut s, ROW_BROWSER);
    let out = render(&mut s);
    for label in ["Google", "GitHub", "X (Twitter)"] {
        assert!(out.contains(label), "{label} offered: {out}");
    }
    assert!(
        !out.contains("Discord"),
        "the backend has no Discord login: {out}"
    );
}

#[test]
fn each_provider_row_starts_loopback_with_that_provider() {
    for (row, provider) in [Provider::Google, Provider::Github, Provider::Twitter]
        .into_iter()
        .enumerate()
    {
        let mut s = LoginScreen::new("http://b");
        let cmd = sign_in(&mut s, ROW_BROWSER, row);
        assert_eq!(
            cmd,
            Some(LoginCmd::StartLoopback {
                base_url: "http://b".into(),
                provider,
            }),
            "row {row} signs in with {provider:?}"
        );
        // The choice sticks, so a retry after a failure reuses it.
        assert_eq!(s.provider(), provider);
        assert_eq!(s.method(), SignInMethod::Browser);
    }
}

#[test]
fn the_code_method_shows_a_verification_url_and_needs_no_listener() {
    let mut s = LoginScreen::new("http://b/");
    // No async command: the URL is a pure function of the backend and provider,
    // and there is deliberately no loopback listener to bind — over SSH the
    // callback could never reach one.
    assert_eq!(sign_in(&mut s, ROW_CODE, 1), None);
    assert_eq!(s.method(), SignInMethod::Code);
    assert_eq!(s.provider(), Provider::Github);

    let out = render(&mut s);
    assert!(
        out.contains("http://b/auth/github/login?redirect=cli"),
        "verification URL: {out}"
    );
    assert!(out.contains("any device"), "open-anywhere hint: {out}");
    assert!(
        !out.contains("127.0.0.1"),
        "nothing is bound locally: {out}"
    );
}

#[test]
fn a_code_is_typed_or_pasted_and_submitted_for_redemption() {
    let mut s = LoginScreen::new("b");
    sign_in(&mut s, ROW_CODE, 0);

    // Empty submit is refused with an error, no command.
    assert!(s.handle_key(key(KeyCode::Enter)).is_none());
    assert!(render(&mut s).contains("paste the code"));

    // Bracketed paste delivers the code in one event, trailing newline and all;
    // it must not submit itself.
    s.handle_paste(&format!("{}\n", "a".repeat(64)));
    assert!(render(&mut s).contains("aaaa"), "code echoed");
    assert_eq!(
        s.handle_key(key(KeyCode::Enter)),
        Some(LoginCmd::SubmitToken("a".repeat(64))),
        "the newline is trimmed and the code goes out whole"
    );
}

#[test]
fn ctrl_o_offers_to_open_the_verification_url_without_eating_typed_characters() {
    let mut s = LoginScreen::new("http://b");
    sign_in(&mut s, ROW_CODE, 0);

    // A plain 'o' belongs to the code being entered, so the escape hatch has to
    // be a chord.
    assert!(s.handle_key(key(KeyCode::Char('o'))).is_none());
    let cmd = s.handle_key(KeyEvent::new(KeyCode::Char('o'), KeyModifiers::CONTROL));
    assert_eq!(
        cmd,
        Some(LoginCmd::OpenUrl(
            "http://b/auth/google/login?redirect=cli".into()
        ))
    );
    // The typed 'o' is still in the field.
    assert_eq!(
        s.handle_key(key(KeyCode::Enter)),
        Some(LoginCmd::SubmitToken("o".into()))
    );
}

#[test]
fn a_rejected_code_returns_to_the_code_screen_with_the_url_still_up() {
    let mut s = LoginScreen::new("http://b");
    sign_in(&mut s, ROW_CODE, 0);
    s.handle_paste("expired");
    s.handle_key(key(KeyCode::Enter));
    s.apply(LoginEvent::VerifyFailed(
        "login token redemption failed".into(),
    ));

    let out = render(&mut s);
    assert!(
        out.contains("login token redemption failed"),
        "error: {out}"
    );
    // Fetching another code needs the URL, so the screen must not fall back to
    // the menu where it is no longer shown.
    assert!(out.contains("redirect=cli"), "URL still shown: {out}");
}

#[test]
fn esc_from_the_code_screen_returns_to_the_provider_list() {
    let mut s = LoginScreen::new("b");
    sign_in(&mut s, ROW_CODE, 0);
    s.handle_key(key(KeyCode::Esc));
    let out = render(&mut s);
    assert!(out.contains("choose a provider"), "provider list: {out}");
    // And Esc again is the way back to the menu.
    s.handle_key(key(KeyCode::Esc));
    assert!(render(&mut s).contains("Sign in with a browser"));
}

#[test]
fn the_menus_wrap_at_both_ends() {
    // The lists are short and every row is reachable both ways, so overshooting
    // should not mean travelling back through the whole menu.
    let mut s = LoginScreen::new("b");
    s.handle_key(key(KeyCode::Up)); // wrap up from the first row to the last
    assert!(s.handle_key(key(KeyCode::Enter)).is_none());
    assert_eq!(s.outcome(), Some(LoginOutcome::Quit));

    let mut p = LoginScreen::new("b");
    choose(&mut p, ROW_BROWSER);
    p.handle_key(key(KeyCode::Up));
    assert!(matches!(
        p.handle_key(key(KeyCode::Enter)),
        Some(LoginCmd::StartLoopback { .. })
    ));
    assert_eq!(p.provider(), Provider::Twitter, "wrapped to the last row");
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
    sign_in(&mut s, ROW_BROWSER, 0);
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
        choose(&mut s, ROW_PASTE_KEY).is_none(),
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
    choose(&mut s2, ROW_PASTE_KEY);
    assert!(s2.handle_key(key(KeyCode::Enter)).is_none());
    assert!(render(&mut s2).contains("enter a token"));
}

#[test]
fn a_pasted_token_lands_in_the_input_and_waits_for_enter() {
    let mut s = LoginScreen::new("b");
    choose(&mut s, ROW_PASTE_KEY);

    // Bracketed paste delivers the key in one event, trailing newline and all.
    // It must not submit itself — the operator still presses Enter.
    s.handle_paste("sk-test-key\n");
    assert!(s.handle_key(key(KeyCode::Enter)).is_some());

    let mut again = LoginScreen::new("b");
    choose(&mut again, ROW_PASTE_KEY);
    again.handle_key(key(KeyCode::Char('s')));
    again.handle_paste("k-rest\r\n");
    assert_eq!(
        again.handle_key(key(KeyCode::Enter)),
        Some(LoginCmd::SubmitToken("sk-rest".into())),
        "the paste appends to what was typed and the newline is trimmed off"
    );
}

#[test]
fn a_paste_outside_an_input_phase_is_ignored() {
    let mut s = LoginScreen::new("b");
    s.handle_paste("stray");
    // Still on the menu: opening token entry now shows an empty field.
    choose(&mut s, ROW_PASTE_KEY);
    assert!(s.handle_key(key(KeyCode::Enter)).is_none());
    assert!(render(&mut s).contains("enter a token"));
}

#[test]
fn the_quit_row_and_ctrl_c_yield_quit() {
    let mut q = LoginScreen::new("b");
    choose(&mut q, ROW_QUIT);
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
    // Still usable: a failure with no input phase behind it lands on the menu,
    // which can start over.
    assert!(s.handle_key(key(KeyCode::Enter)).is_none());
    assert!(render(&mut s).contains("choose a provider"));

    let mut s2 = LoginScreen::new("b");
    s2.apply(LoginEvent::CallbackError("state mismatch timeout".into()));
    assert!(render(&mut s2).contains("state mismatch timeout"));
}

#[test]
fn tick_advances_spinner_without_panic() {
    let mut s = LoginScreen::new("b");
    sign_in(&mut s, ROW_BROWSER, 0);
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
