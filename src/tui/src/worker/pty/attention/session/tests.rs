//! Unit tests for the session-lifecycle attention cues.
//!
//! These cover [`super::lifecycle_cue`] and [`super::row_cue`]: the cues a row
//! derives from a session's *life* (it died, or a dispatched turn finished)
//! rather than from the screen it painted. Housed in this module's own
//! canonical `tests.rs`, as the directory-module layout requires.

use medulla::protocol::HarnessProvider;

use super::super::super::types::{PtyState, SessionControl, SessionOrigin, SessionRow};
use super::super::types::{AttentionKind, HarnessAttention};
use super::{lifecycle_cue, row_cue};

/// Epoch ms used for every lifecycle cue stamp, so elapsed times are exact.
const LIFECYCLE_NOW: i64 = 1_700_000_000_000;

/// A healthy running session with nothing to say.
fn lifecycle_row() -> SessionRow {
    SessionRow {
        id: "w-1".into(),
        label: "codex".into(),
        provider: HarnessProvider::Codex,
        preset: None,
        state: PtyState::Running,
        cwd: "/workspace/medulla".into(),
        checkout: Default::default(),
        launch_root: None,
        launch_commit: None,
        launch_checkout_identity: None,
        session_id: None,
        thread_name: None,
        started_at: LIFECYCLE_NOW,
        last_output_at: LIFECYCLE_NOW,
        last_error: None,
        busy: false,
        control: SessionControl::User,
        origin: SessionOrigin::User,
        retained: false,
        name: None,
        attention: None,
        working: false,
        mcp_grant_session: None,
    }
}

#[test]
fn a_healthy_running_session_has_nothing_to_report() {
    assert_eq!(lifecycle_cue(&lifecycle_row(), LIFECYCLE_NOW), None);
    assert_eq!(row_cue(&lifecycle_row(), LIFECYCLE_NOW), None);
}

#[test]
fn a_clean_exit_is_not_a_failure() {
    let mut row = lifecycle_row();
    row.state = PtyState::Exited { code: Some(0) };
    assert_eq!(lifecycle_cue(&row, LIFECYCLE_NOW), None);
}

#[test]
fn a_non_zero_exit_reports_its_code() {
    let mut row = lifecycle_row();
    row.state = PtyState::Exited { code: Some(137) };
    let cue = lifecycle_cue(&row, LIFECYCLE_NOW).expect("a cue");
    assert_eq!(cue.kind, AttentionKind::Failed);
    assert!(cue.what.contains("137"), "{}", cue.what);
}

#[test]
fn lifecycle_cues_keep_their_original_timestamp() {
    let mut row = lifecycle_row();
    row.state = PtyState::Exited { code: Some(137) };
    row.last_output_at = LIFECYCLE_NOW + 5_000;

    let cue = lifecycle_cue(&row, LIFECYCLE_NOW + 60_000).expect("a cue");
    assert_eq!(cue.since, row.last_output_at);
}

#[test]
fn a_pty_that_never_started_reports_a_failure() {
    let mut row = lifecycle_row();
    row.state = PtyState::Failed;
    assert_eq!(
        lifecycle_cue(&row, LIFECYCLE_NOW).expect("a cue").kind,
        AttentionKind::Failed
    );
}

#[test]
fn a_recorded_error_outranks_the_exit_status() {
    let mut row = lifecycle_row();
    row.state = PtyState::Exited { code: Some(1) };
    row.last_error = Some("write queue full".into());
    let cue = lifecycle_cue(&row, LIFECYCLE_NOW).expect("a cue");
    assert!(cue.what.contains("write queue full"), "{}", cue.what);
    assert!(!cue.what.contains("exited with"), "{}", cue.what);
}

#[test]
fn a_retained_session_asks_to_be_read_and_released() {
    let mut row = lifecycle_row();
    row.retained = true;
    let cue = lifecycle_cue(&row, LIFECYCLE_NOW).expect("a cue");
    assert_eq!(cue.kind, AttentionKind::Completed);
    assert!(cue.what.contains("finished"), "{}", cue.what);
}

#[test]
fn a_retained_session_that_failed_reports_the_failure() {
    let mut row = lifecycle_row();
    row.retained = true;
    row.state = PtyState::Failed;
    assert_eq!(
        lifecycle_cue(&row, LIFECYCLE_NOW).expect("a cue").kind,
        AttentionKind::Failed
    );
}

#[test]
fn a_screen_cue_does_not_outlive_the_child_that_painted_it() {
    let mut row = lifecycle_row();
    row.attention = Some(HarnessAttention::new(
        AttentionKind::Approval,
        "codex is asking permission",
        LIFECYCLE_NOW,
    ));
    row.state = PtyState::Exited { code: Some(0) };
    assert_eq!(row_cue(&row, LIFECYCLE_NOW), None);
}

#[test]
fn a_live_session_keeps_the_cue_its_screen_produced() {
    let mut row = lifecycle_row();
    row.attention = Some(HarnessAttention::new(
        AttentionKind::Approval,
        "codex is asking permission",
        LIFECYCLE_NOW,
    ));
    assert_eq!(
        row_cue(&row, LIFECYCLE_NOW).expect("a cue").kind,
        AttentionKind::Approval
    );
}

#[test]
fn a_failure_outranks_the_screen() {
    let mut row = lifecycle_row();
    row.attention = Some(HarnessAttention::new(
        AttentionKind::Choice,
        "codex is waiting on a choice",
        LIFECYCLE_NOW,
    ));
    row.state = PtyState::Failed;
    assert_eq!(
        row_cue(&row, LIFECYCLE_NOW).expect("a cue").kind,
        AttentionKind::Failed
    );
}
