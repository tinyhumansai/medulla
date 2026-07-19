//! Unit tests for the welcome screen's state machine.
//!
//! The consent gate carries the most weight: the upload command must be
//! unreachable except through an explicit approval on the consent step.

use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyEventState, KeyModifiers};

use super::types::{ScanSummary, Step, WelcomeCmd, WelcomeEvent, WelcomeOutcome, WelcomeScreen};

fn key(code: KeyCode) -> KeyEvent {
    KeyEvent {
        code,
        modifiers: KeyModifiers::NONE,
        kind: KeyEventKind::Press,
        state: KeyEventState::NONE,
    }
}

fn ctrl(code: KeyCode) -> KeyEvent {
    KeyEvent {
        code,
        modifiers: KeyModifiers::CONTROL,
        kind: KeyEventKind::Press,
        state: KeyEventState::NONE,
    }
}

fn found(session_count: usize) -> ScanSummary {
    ScanSummary {
        per_agent: vec![("claude".into(), session_count)],
        session_count,
        total_bytes: 4096,
        skipped_oversize: 0,
    }
}

/// A screen parked directly at `step`, for testing transitions in isolation.
fn screen_at(step: Step) -> WelcomeScreen {
    WelcomeScreen {
        step,
        ..WelcomeScreen::default()
    }
}

fn screen_at_consent() -> WelcomeScreen {
    let mut screen = WelcomeScreen::default();
    assert_eq!(
        screen.handle_key(key(KeyCode::Enter)),
        Some(WelcomeCmd::Scan)
    );
    screen.apply(WelcomeEvent::ScanReady(found(12)));
    assert_eq!(screen.step, Step::Consent);
    screen
}

#[test]
fn intro_enter_starts_a_scan() {
    let mut screen = WelcomeScreen::default();

    assert_eq!(
        screen.handle_key(key(KeyCode::Enter)),
        Some(WelcomeCmd::Scan)
    );
    assert_eq!(screen.step, Step::Scanning);
    assert!(screen.outcome().is_none());
}

#[test]
fn intro_can_be_skipped() {
    for code in [KeyCode::Esc, KeyCode::Char('q')] {
        let mut screen = WelcomeScreen::default();
        assert_eq!(screen.handle_key(key(code)), None);
        assert_eq!(screen.outcome(), Some(WelcomeOutcome::Skipped));
    }
}

#[test]
fn ctrl_c_skips_from_any_step() {
    let steps = [Step::Intro, Step::Scanning, Step::Consent, Step::Uploading];
    for step in steps {
        let mut screen = screen_at(step);
        assert_eq!(screen.handle_key(ctrl(KeyCode::Char('c'))), None);
        assert_eq!(
            screen.outcome(),
            Some(WelcomeOutcome::Skipped),
            "ctrl-c should skip from {step:?}"
        );
    }
}

#[test]
fn a_scan_that_finds_sessions_advances_to_consent() {
    let mut screen = WelcomeScreen::default();
    screen.handle_key(key(KeyCode::Enter));

    screen.apply(WelcomeEvent::ScanReady(found(3)));

    assert_eq!(screen.step, Step::Consent);
    assert_eq!(screen.scan.session_count, 3);
}

#[test]
fn a_scan_that_finds_nothing_lands_on_the_empty_step() {
    let mut screen = WelcomeScreen::default();
    screen.handle_key(key(KeyCode::Enter));

    screen.apply(WelcomeEvent::ScanReady(ScanSummary::default()));

    assert_eq!(screen.step, Step::Empty);
    // Leaving from there is neither a claim nor a skip: the offer stays open.
    assert_eq!(screen.handle_key(key(KeyCode::Enter)), None);
    assert_eq!(screen.outcome(), Some(WelcomeOutcome::NothingToShare));
}

#[test]
fn consent_enter_is_the_only_source_of_an_upload() {
    // Every step other than Consent, and every key other than Enter on Consent,
    // must never produce an upload command.
    let non_consent = [
        Step::Intro,
        Step::Scanning,
        Step::Uploading,
        Step::Reveal,
        Step::Empty,
    ];
    let codes = [
        KeyCode::Enter,
        KeyCode::Esc,
        KeyCode::Char('q'),
        KeyCode::Char('y'),
        KeyCode::Backspace,
    ];

    for step in non_consent {
        for code in codes {
            let mut screen = screen_at(step);
            let cmd = screen.handle_key(key(code));
            assert_ne!(
                cmd,
                Some(WelcomeCmd::UploadAndClaim),
                "{step:?} + {code:?} must not upload"
            );
        }
    }

    for code in [KeyCode::Esc, KeyCode::Char('q'), KeyCode::Char('y')] {
        let mut screen = screen_at_consent();
        assert_ne!(
            screen.handle_key(key(code)),
            Some(WelcomeCmd::UploadAndClaim),
            "consent + {code:?} must not upload"
        );
    }

    let mut screen = screen_at_consent();
    assert_eq!(
        screen.handle_key(key(KeyCode::Enter)),
        Some(WelcomeCmd::UploadAndClaim)
    );
    assert_eq!(screen.step, Step::Uploading);
}

#[test]
fn declining_consent_skips_without_uploading() {
    let mut screen = screen_at_consent();

    assert_eq!(screen.handle_key(key(KeyCode::Esc)), None);
    assert_eq!(screen.outcome(), Some(WelcomeOutcome::Skipped));
}

#[test]
fn consent_seeds_the_upload_total_from_the_scan() {
    let mut screen = screen_at_consent();

    screen.handle_key(key(KeyCode::Enter));

    assert_eq!(screen.upload_total, 12);
    assert_eq!(screen.uploaded, 0);
}

#[test]
fn upload_progress_is_folded_into_state() {
    let mut screen = screen_at_consent();
    screen.handle_key(key(KeyCode::Enter));

    screen.apply(WelcomeEvent::UploadProgress {
        uploaded: 5,
        total: 12,
        redactions: 3,
    });

    assert_eq!(screen.uploaded, 5);
    assert_eq!(screen.upload_total, 12);
    assert_eq!(screen.redactions, 3);
    assert_eq!(screen.step, Step::Uploading);
}

#[test]
fn claiming_reveals_the_award() {
    let mut screen = screen_at_consent();
    screen.handle_key(key(KeyCode::Enter));

    screen.apply(WelcomeEvent::Claimed {
        awarded_usd: 7.0,
        tier: Some("Rising".into()),
        breakdown: vec![("token volume".into(), 2.0), ("multi-agent".into(), 3.0)],
        max_reward_usd: 25.0,
        already_claimed: false,
    });

    assert_eq!(screen.step, Step::Reveal);
    assert_eq!(screen.awarded_usd, 7.0);
    assert_eq!(screen.tier.as_deref(), Some("Rising"));
    assert_eq!(screen.max_reward_usd, 25.0);
    assert!(!screen.already_claimed);

    assert_eq!(screen.handle_key(key(KeyCode::Enter)), None);
    assert_eq!(
        screen.outcome(),
        Some(WelcomeOutcome::Completed {
            awarded_usd: 7.0,
            tier: Some("Rising".into()),
        })
    );
}

#[test]
fn a_repeat_claim_is_reported_as_already_claimed() {
    let mut screen = screen_at(Step::Uploading);

    screen.apply(WelcomeEvent::Claimed {
        awarded_usd: 12.0,
        tier: Some("Elite".into()),
        breakdown: Vec::new(),
        max_reward_usd: 25.0,
        already_claimed: true,
    });

    assert!(screen.already_claimed);
    assert_eq!(screen.awarded_usd, 12.0);
}

#[test]
fn a_failure_lands_on_the_reveal_with_the_message() {
    let mut screen = screen_at(Step::Uploading);

    screen.apply(WelcomeEvent::Failed("backend unreachable".into()));

    assert_eq!(screen.step, Step::Reveal);
    assert_eq!(screen.error.as_deref(), Some("backend unreachable"));

    // The user can still leave — but as Unavailable, not Completed. The reward
    // never settled, so the caller must keep the offer open rather than
    // recording onboarding as done over a transient failure.
    screen.handle_key(key(KeyCode::Enter));
    assert_eq!(screen.outcome(), Some(WelcomeOutcome::Unavailable));
}

#[test]
fn a_zero_max_reward_falls_back_to_the_default() {
    let screen = WelcomeScreen::new(0.0);
    assert_eq!(screen.max_reward_usd, super::types::DEFAULT_MAX_REWARD_USD);
}

#[test]
fn the_backend_ceiling_overrides_the_default_on_claim() {
    let mut screen = screen_at(Step::Uploading);

    screen.apply(WelcomeEvent::Claimed {
        awarded_usd: 5.0,
        tier: None,
        breakdown: Vec::new(),
        max_reward_usd: 40.0,
        already_claimed: false,
    });

    assert_eq!(screen.max_reward_usd, 40.0);
}

#[test]
fn the_spinner_advances_on_tick() {
    let mut screen = WelcomeScreen::default();
    let first = screen.spinner();
    screen.tick();
    assert_ne!(first, screen.spinner());
}

#[test]
fn a_successful_claim_still_completes() {
    // The counterpart to the failure case: no error means the reward settled,
    // so onboarding is genuinely done.
    let mut screen = screen_at(Step::Uploading);
    screen.apply(WelcomeEvent::Claimed {
        awarded_usd: 9.0,
        tier: Some("Seasoned".into()),
        breakdown: Vec::new(),
        max_reward_usd: 25.0,
        already_claimed: false,
    });

    screen.handle_key(key(KeyCode::Enter));

    assert_eq!(
        screen.outcome(),
        Some(WelcomeOutcome::Completed {
            awarded_usd: 9.0,
            tier: Some("Seasoned".into()),
        })
    );
}
