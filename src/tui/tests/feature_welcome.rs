//! Feature tests for the first-run welcome screen
//! ([`medulla_tui::ui::welcome`]) and the redaction it depends on
//! ([`medulla::history_upload`]): pure rendering and key/event transitions
//! driven entirely through the public API (no async, no TTY, no network).
//!
//! The consent panel gets the most scrutiny. It is the only place a user is told
//! what leaves their machine, so its promises are asserted here rather than left
//! to review.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::backend::TestBackend;
use ratatui::Terminal;

use medulla::history_upload::redact_text;
use medulla_tui::ui::welcome::{
    format_usd, ScanSummary, WelcomeCmd, WelcomeEvent, WelcomeOutcome, WelcomeScreen,
};

fn key(code: KeyCode) -> KeyEvent {
    KeyEvent::new(code, KeyModifiers::NONE)
}

/// Render the screen into a test terminal and flatten the buffer to text.
fn render(screen: &mut WelcomeScreen) -> String {
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

fn scan(session_count: usize) -> ScanSummary {
    ScanSummary {
        per_agent: vec![("claude".into(), session_count), ("codex".into(), 4)],
        session_count,
        total_bytes: 2 * 1024 * 1024,
        skipped_oversize: 0,
    }
}

/// Drive a fresh screen to the consent step.
fn at_consent() -> WelcomeScreen {
    let mut screen = WelcomeScreen::default();
    screen.handle_key(key(KeyCode::Enter));
    screen.apply(WelcomeEvent::ScanReady(scan(20)));
    screen
}

#[test]
fn intro_pitches_the_reward_and_the_privacy_terms() {
    let mut screen = WelcomeScreen::default();
    let out = render(&mut screen);

    assert!(out.contains("MEDULLA"), "branding: {out}");
    assert!(out.contains("earn up to $25"), "the offer: {out}");
    assert!(out.contains("power user"), "the pitch: {out}");
    assert!(
        out.contains("secrets are stripped before anything is sent"),
        "privacy promise up front: {out}"
    );
    assert!(
        out.contains("you approve the exact upload first"),
        "consent promised up front: {out}"
    );
    assert!(out.contains("Esc to skip"), "escape hatch: {out}");
}

#[test]
fn scanning_states_that_nothing_has_been_sent() {
    let mut screen = WelcomeScreen::default();
    screen.handle_key(key(KeyCode::Enter));

    let out = render(&mut screen);
    assert!(out.contains("scanning local sessions"), "progress: {out}");
    assert!(
        out.contains("nothing has been sent yet"),
        "reassurance while reading local files: {out}"
    );
}

#[test]
fn consent_shows_exactly_what_would_be_uploaded() {
    let mut screen = at_consent();
    let out = render(&mut screen);

    assert!(out.contains("claude"), "per-agent row: {out}");
    assert!(out.contains("codex"), "per-agent row: {out}");
    assert!(out.contains("20 sessions"), "total count: {out}");
    assert!(out.contains("2.0 MB"), "size estimate: {out}");
    assert!(
        out.contains("API keys, tokens, and passwords are removed before sending."),
        "redaction promise: {out}"
    );
    assert!(
        out.contains("Transcripts are encrypted at rest."),
        "encryption promise: {out}"
    );
    assert!(out.contains("Esc to skip"), "escape hatch: {out}");
}

#[test]
fn consent_warns_about_skipped_oversize_sessions() {
    let mut screen = WelcomeScreen::default();
    screen.handle_key(key(KeyCode::Enter));
    screen.apply(WelcomeEvent::ScanReady(ScanSummary {
        per_agent: vec![("claude".into(), 2)],
        session_count: 2,
        total_bytes: 1024,
        skipped_oversize: 3,
    }));

    let out = render(&mut screen);
    assert!(
        out.contains("3 oversized session(s) will be skipped"),
        "oversize note: {out}"
    );
}

#[test]
fn uploading_shows_progress_and_the_redaction_count() {
    let mut screen = at_consent();
    screen.handle_key(key(KeyCode::Enter));
    screen.apply(WelcomeEvent::UploadProgress {
        uploaded: 8,
        total: 20,
        redactions: 4,
    });

    let out = render(&mut screen);
    assert!(out.contains("8/20"), "progress counter: {out}");
    assert!(out.contains('█'), "filled meter: {out}");
    assert!(out.contains('░'), "unfilled meter: {out}");
    assert!(
        out.contains("4 secret(s) scrubbed before sending"),
        "redaction count: {out}"
    );
}

#[test]
fn the_reveal_shows_power_level_award_and_breakdown() {
    let mut screen = at_consent();
    screen.handle_key(key(KeyCode::Enter));
    screen.apply(WelcomeEvent::Claimed {
        awarded_usd: 7.0,
        tier: Some("Rising".into()),
        breakdown: vec![
            ("token volume".into(), 2.0),
            ("active days".into(), 2.0),
            ("sessions".into(), 0.0),
            ("multi-agent".into(), 3.0),
        ],
        max_reward_usd: 25.0,
        already_claimed: false,
    });

    let out = render(&mut screen);
    assert!(out.contains("POWER LEVEL"), "tier label: {out}");
    assert!(out.contains("RISING"), "tier value: {out}");
    assert!(out.contains("$7 of $25 earned"), "award: {out}");
    assert!(out.contains("token volume"), "breakdown row: {out}");
    assert!(out.contains("multi-agent"), "breakdown row: {out}");
    assert!(
        !out.contains("sessions          $0"),
        "zero rows are omitted: {out}"
    );
    assert!(
        out.contains("Credit has been added to your balance."),
        "confirmation: {out}"
    );
}

#[test]
fn a_zero_award_is_reported_kindly() {
    let mut screen = at_consent();
    screen.handle_key(key(KeyCode::Enter));
    screen.apply(WelcomeEvent::Claimed {
        awarded_usd: 0.0,
        tier: Some("Newcomer".into()),
        breakdown: Vec::new(),
        max_reward_usd: 25.0,
        already_claimed: false,
    });

    let out = render(&mut screen);
    assert!(out.contains("$0 of $25 earned"), "award: {out}");
    assert!(
        out.contains("come back once you've logged more sessions"),
        "encouraging dead end: {out}"
    );
}

#[test]
fn a_repeat_claim_says_no_new_credit_was_added() {
    let mut screen = at_consent();
    screen.handle_key(key(KeyCode::Enter));
    screen.apply(WelcomeEvent::Claimed {
        awarded_usd: 12.0,
        tier: Some("Elite".into()),
        breakdown: Vec::new(),
        max_reward_usd: 25.0,
        already_claimed: true,
    });

    let out = render(&mut screen);
    assert!(
        out.contains("already claimed — no new credit was added"),
        "repeat-claim note: {out}"
    );
}

#[test]
fn the_empty_state_explains_why_there_is_nothing_to_share() {
    let mut screen = WelcomeScreen::default();
    screen.handle_key(key(KeyCode::Enter));
    screen.apply(WelcomeEvent::ScanReady(ScanSummary::default()));

    let out = render(&mut screen);
    assert!(out.contains("No local history found"), "heading: {out}");
    assert!(
        out.contains("Use an agent for a"),
        "what to do about it: {out}"
    );
}

#[test]
fn a_failure_is_surfaced_on_screen() {
    let mut screen = at_consent();
    screen.handle_key(key(KeyCode::Enter));
    screen.apply(WelcomeEvent::Failed("backend unreachable".into()));

    let out = render(&mut screen);
    assert!(out.contains("error: backend unreachable"), "error: {out}");
}

#[test]
fn the_happy_path_runs_intro_to_completion() {
    let mut screen = WelcomeScreen::default();

    // Intro → scan.
    assert_eq!(
        screen.handle_key(key(KeyCode::Enter)),
        Some(WelcomeCmd::Scan)
    );
    screen.apply(WelcomeEvent::ScanReady(scan(20)));

    // Consent → upload. This is the only approval point.
    assert_eq!(
        screen.handle_key(key(KeyCode::Enter)),
        Some(WelcomeCmd::UploadAndClaim)
    );
    screen.apply(WelcomeEvent::UploadProgress {
        uploaded: 20,
        total: 20,
        redactions: 2,
    });
    screen.apply(WelcomeEvent::Claimed {
        awarded_usd: 7.0,
        tier: Some("Rising".into()),
        breakdown: Vec::new(),
        max_reward_usd: 25.0,
        already_claimed: false,
    });

    // Reveal → done.
    screen.handle_key(key(KeyCode::Enter));
    assert_eq!(
        screen.outcome(),
        Some(WelcomeOutcome::Completed {
            awarded_usd: 7.0,
            tier: Some("Rising".into()),
        })
    );
}

#[test]
fn skipping_at_the_intro_never_reaches_an_upload() {
    let mut screen = WelcomeScreen::default();

    assert_eq!(screen.handle_key(key(KeyCode::Esc)), None);
    assert_eq!(screen.outcome(), Some(WelcomeOutcome::Skipped));
}

#[test]
fn redaction_and_the_consent_promise_agree() {
    // The consent panel promises API keys, tokens, and passwords are removed.
    // Assert the redactor actually delivers on each of those three claims, so
    // the copy can never drift away from the behaviour.
    let cases = [
        r#"{"t":"api_key=supersecretvalue123"}"#,
        r#"{"t":"ACCESS_TOKEN=abcdefgh12345678"}"#,
        r#"{"t":"password: hunter2hunter2"}"#,
    ];
    for case in cases {
        let (out, count) = redact_text(case);
        assert!(count >= 1, "nothing redacted in {case}");
        assert!(out.contains("[REDACTED]"), "no placeholder in {out}");
    }

    // And a transcript's scoring metadata survives, so the award is honest.
    let metadata = r#"{"timestamp":"2026-01-05T10:00:00Z","message":{"usage":{"input_tokens":100000,"output_tokens":50000}}}"#;
    let (out, count) = redact_text(metadata);
    assert_eq!(out, metadata);
    assert_eq!(count, 0);
}

#[test]
fn usd_formatting_matches_between_the_panel_and_the_status_line() {
    assert_eq!(format_usd(25.0), "$25");
    assert_eq!(format_usd(0.0), "$0");
    assert_eq!(format_usd(7.5), "$7.50");
}
