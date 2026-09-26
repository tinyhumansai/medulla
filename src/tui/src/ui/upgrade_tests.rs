//! Unit tests for the subscription gate screen's state machine.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use medulla::access::Denial;

use super::*;

fn key(code: KeyCode) -> KeyEvent {
    KeyEvent::new(code, KeyModifiers::NONE)
}

fn screen() -> UpgradeScreen {
    UpgradeScreen::new("Logged in as a@b.c (u1)", Denial::TrialExpired)
}

#[test]
fn it_starts_with_no_outcome() {
    assert_eq!(screen().outcome(), None);
}

/// Down from Subscribe is Check again, not Quit — the row an operator who has
/// just paid reaches for.
#[test]
fn the_menu_offers_a_re_check_and_a_sign_out() {
    let out = render(&mut screen());
    assert!(out.contains("Check again"), "the re-check row is missing");
    assert!(out.contains("Sign out"), "the sign-out row is missing");
}

#[test]
fn checking_again_asks_the_caller_to_re_ask_the_backend() {
    let mut s = screen();

    s.handle_key(key(KeyCode::Down));
    assert_eq!(s.handle_key(key(KeyCode::Enter)), None);

    // Says so before the caller blocks on the network, or the screen reads as
    // frozen for the length of the round trip.
    assert!(render(&mut s).contains("Checking your subscription"));
    assert_eq!(s.take_outcome(), Some(UpgradeOutcome::CheckAgain));
}

#[test]
fn r_is_a_shortcut_for_checking_again() {
    let mut s = screen();
    s.handle_key(key(KeyCode::Char('r')));
    assert_eq!(s.take_outcome(), Some(UpgradeOutcome::CheckAgain));
}

/// The caller takes the outcome, acts, and hands the screen back. Peeking with
/// `outcome()` instead would leave `CheckAgain` set and re-check forever.
#[test]
fn taking_the_outcome_leaves_the_screen_running() {
    let mut s = screen();
    s.handle_key(key(KeyCode::Char('r')));

    assert_eq!(s.take_outcome(), Some(UpgradeOutcome::CheckAgain));
    assert_eq!(s.take_outcome(), None);

    s.rechecked("Logged in as a@b.c (u1)", Denial::TrialExpired);
    assert_eq!(s.outcome(), None);
    assert!(render(&mut s).contains("still has no subscription"));
}

/// A re-check that reached the backend may learn something new — a plan that
/// went from unreadable to confirmed free — and the panel has to follow it.
#[test]
fn a_re_check_adopts_the_reason_the_backend_now_gives() {
    let mut s = UpgradeScreen::new("backend https://api.tinyhumans.ai", Denial::Undetermined);
    assert!(render(&mut s).contains("could not be confirmed"));

    s.rechecked("Logged in as a@b.c (u1)", Denial::TrialExpired);

    let out = render(&mut s);
    assert!(out.contains("Your free trial has ended."));
    assert!(
        out.contains("a@b.c"),
        "the freshly read account is not named"
    );
}

/// A re-check that never reached the backend learned nothing about the account,
/// so it must not rewrite the headline as if it had.
#[test]
fn a_failed_re_check_reports_itself_without_changing_the_verdict() {
    let mut s = screen();

    s.recheck_failed("connection refused");

    let out = render(&mut s);
    assert!(out.contains("Your free trial has ended."));
    assert!(out.contains("connection refused"));
}

#[test]
fn the_sign_out_row_asks_the_caller_to_end_the_session() {
    let mut s = screen();

    for _ in 0..2 {
        s.handle_key(key(KeyCode::Down));
    }
    s.handle_key(key(KeyCode::Enter));

    assert_eq!(s.outcome(), Some(UpgradeOutcome::SignOut));
}

#[test]
fn subscribe_opens_the_pricing_page_without_leaving() {
    let mut s = screen();

    let cmd = s.handle_key(key(KeyCode::Enter));

    assert_eq!(cmd, Some(UpgradeCmd::OpenUrl(PRICING_URL.to_string())));
    // Opening the page is not a way out of the screen: the account still has no
    // plan until the checkout completes and the next launch re-checks.
    assert_eq!(s.outcome(), None);
}

#[test]
fn the_quit_row_leaves() {
    let mut s = screen();

    for _ in 0..3 {
        assert_eq!(s.handle_key(key(KeyCode::Down)), None);
    }
    assert_eq!(s.handle_key(key(KeyCode::Enter)), None);

    assert_eq!(s.outcome(), Some(UpgradeOutcome::Quit));
}

#[test]
fn esc_leaves_because_there_is_nothing_to_back_out_to() {
    let mut s = screen();
    s.handle_key(key(KeyCode::Esc));
    assert_eq!(s.outcome(), Some(UpgradeOutcome::Quit));
}

#[test]
fn ctrl_c_leaves_from_any_row() {
    let mut s = screen();
    s.handle_key(key(KeyCode::Down));
    s.handle_key(key(KeyCode::Down));
    s.handle_key(KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL));
    assert_eq!(s.outcome(), Some(UpgradeOutcome::Quit));
}

#[test]
fn the_selection_wraps_in_both_directions() {
    let mut s = screen();

    // Up from the first row lands on Quit, which Enter then takes.
    s.handle_key(key(KeyCode::Up));
    s.handle_key(key(KeyCode::Enter));

    assert_eq!(s.outcome(), Some(UpgradeOutcome::Quit));
}

/// There is no key that admits the operator to the app on its own. `CheckAgain`
/// is not an exception: it asks the *caller* to re-run the policy against a
/// fresh `/auth/me`, and the account is admitted only if that says yes. If a key
/// is ever wired straight into the app, this fails — which is the point.
#[test]
fn no_keystroke_dismisses_the_screen_into_the_app() {
    let codes = [
        KeyCode::Tab,
        KeyCode::Backspace,
        KeyCode::Char('y'),
        KeyCode::Char(' '),
        KeyCode::Left,
        KeyCode::Right,
        KeyCode::Home,
    ];
    for code in codes {
        let mut s = screen();
        s.handle_key(key(code));
        assert!(
            matches!(s.outcome(), None | Some(UpgradeOutcome::Quit)),
            "{code:?} produced an outcome that is not a plain quit"
        );
    }
}

/// Render the panel at a given terminal size and return its cells as one
/// string.
fn render_at(s: &mut UpgradeScreen, width: u16, height: u16) -> String {
    use ratatui::backend::TestBackend;
    use ratatui::Terminal;

    let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
    terminal.draw(|frame| s.draw(frame)).unwrap();
    terminal
        .backend()
        .buffer()
        .content()
        .iter()
        .map(|cell| cell.symbol())
        .collect()
}

/// Render the panel and return its cells as one string.
fn render(s: &mut UpgradeScreen) -> String {
    render_at(s, 100, 32)
}

#[test]
fn the_panel_says_what_happened_who_it_is_about_and_where_to_go() {
    let out = render(&mut screen());

    assert!(
        out.contains("Your free trial has ended."),
        "the reason is missing"
    );
    assert!(out.contains("a@b.c"), "the account is not named");
    assert!(out.contains(PRICING_URL), "the pricing URL is missing");
    assert!(out.contains("Subscribe"), "the subscribe row is missing");
    assert!(out.contains("subscribe"), "the panel title is missing");
}

/// Being refused is not the same as being told what would fix it. The rule
/// itself is on the panel, in both refusals, and it names the tier and the
/// window the policy actually grants.
#[test]
fn the_panel_states_what_access_requires() {
    for denial in [Denial::TrialExpired, Denial::Undetermined] {
        let out = render(&mut UpgradeScreen::new("Logged in as a@b.c (u1)", denial));
        assert!(
            out.contains("requires a Basic subscription"),
            "{denial:?} does not say a Basic subscription is required"
        );
        assert!(
            out.contains(&format!("first {} days", medulla::access::TRIAL_DAYS)),
            "{denial:?} does not name the free window"
        );
    }
}

/// The sentence is built from the policy constant, so it cannot promise a
/// window the gate does not grant.
#[test]
fn the_requirement_lines_name_the_real_free_window() {
    assert!(requirement_lines()[1].contains("first 30 days"));
}

/// The operator may be a paying customer whose plan we failed to look up.
/// Blaming their trial for our outage would be both wrong and insulting.
#[test]
fn an_unconfirmed_plan_does_not_claim_the_trial_expired() {
    let mut s = UpgradeScreen::new("backend https://api.tinyhumans.ai", Denial::Undetermined);

    let out = render(&mut s);

    assert!(
        out.contains("Your subscription could not be confirmed."),
        "the unconfirmed headline is missing"
    );
    assert!(
        !out.contains("free trial has ended"),
        "an unconfirmed plan must not be reported as a spent trial"
    );
    assert!(
        out.contains("check again"),
        "the operator is not told what to do next"
    );
    // Still the way back to a plan, for somebody who genuinely has none.
    assert!(out.contains(PRICING_URL), "the pricing URL is missing");
}

/// And it says what to do after the browser tab, since the checkout finishing
/// is not what lets the operator in — the next check is.
#[test]
fn the_opened_flash_appears_after_subscribing_and_points_at_the_re_check() {
    let mut s = screen();
    assert!(!render(&mut s).contains("Opened"));

    s.handle_key(key(KeyCode::Enter));

    let out = render(&mut s);
    assert!(out.contains("Opened"));
    assert!(out.contains("Check again"));
}

/// On a standard 80x24 terminal the fixed content (logo, headline, rule,
/// hints, menu, shortcuts) already very nearly fills the panel, so the flash
/// a re-check sets is the row most at risk of falling off the bottom — and
/// since the panel never scrolls, "at risk" means "silently missing," which
/// makes a refused re-check look like it did nothing at all. It must stay
/// visible, even if that means the panel drops the two "Already subscribed?"
/// / "On the wrong account?" hint lines, which only restate the menu rows
/// directly below them.
#[test]
fn the_recheck_flash_stays_visible_on_an_80_by_24_terminal() {
    let mut s = screen();
    s.handle_key(key(KeyCode::Char('r')));

    let out = render_at(&mut s, 80, 24);

    assert!(
        out.contains("Checking your subscription"),
        "the re-check flash was clipped off an 80x24 terminal: {out:?}"
    );

    s.rechecked("Logged in as a@b.c (u1)", Denial::TrialExpired);
    let out = render_at(&mut s, 80, 24);
    assert!(
        out.contains("still has no subscription"),
        "the re-check result was clipped off an 80x24 terminal: {out:?}"
    );
}
