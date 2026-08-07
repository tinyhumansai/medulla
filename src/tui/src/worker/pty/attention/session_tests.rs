//! Unit tests for the cues derived from a session's life rather than its screen.

use medulla::protocol::HarnessProvider;

use super::super::types::{PtyState, SessionControl, SessionOrigin, SessionRow};
use super::{lifecycle_cue, row_cue, AttentionKind, HarnessAttention};

/// Epoch ms used for every stamp here, so elapsed times are exact.
const NOW: i64 = 1_700_000_000_000;

/// A healthy running session with nothing to say.
fn row() -> SessionRow {
    SessionRow {
        id: "w-1".into(),
        label: "codex".into(),
        provider: HarnessProvider::Codex,
        preset: None,
        state: PtyState::Running,
        cwd: "/workspace/medulla".into(),
        branch: None,
        launch_root: None,
        launch_commit: None,
        launch_checkout_identity: None,
        session_id: None,
        thread_name: None,
        started_at: NOW,
        last_output_at: NOW,
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
    assert_eq!(lifecycle_cue(&row(), NOW), None);
    assert_eq!(row_cue(&row(), NOW), None);
}

#[test]
fn a_clean_exit_is_not_a_failure() {
    let mut row = row();
    row.state = PtyState::Exited { code: Some(0) };
    assert_eq!(lifecycle_cue(&row, NOW), None);
}

#[test]
fn a_non_zero_exit_reports_its_code() {
    let mut row = row();
    row.state = PtyState::Exited { code: Some(137) };

    let cue = lifecycle_cue(&row, NOW).expect("a cue");

    assert_eq!(cue.kind, AttentionKind::Failed);
    assert!(cue.what.contains("137"), "{}", cue.what);
}

#[test]
fn a_pty_that_never_started_reports_a_failure() {
    let mut row = row();
    row.state = PtyState::Failed;

    assert_eq!(
        lifecycle_cue(&row, NOW).expect("a cue").kind,
        AttentionKind::Failed
    );
}

/// The recorded error names *what* happened; an exit code can only say that
/// something did.
#[test]
fn a_recorded_error_outranks_the_exit_status() {
    let mut row = row();
    row.state = PtyState::Exited { code: Some(1) };
    row.last_error = Some("write queue full".into());

    let cue = lifecycle_cue(&row, NOW).expect("a cue");

    assert!(cue.what.contains("write queue full"), "{}", cue.what);
    assert!(!cue.what.contains("exited with"), "{}", cue.what);
}

/// The state with no other way of announcing itself: running, unbusy, idle at a
/// composer — and holding a finished task nobody has read.
#[test]
fn a_retained_session_asks_to_be_read_and_released() {
    let mut row = row();
    row.retained = true;

    let cue = lifecycle_cue(&row, NOW).expect("a cue");

    assert_eq!(cue.kind, AttentionKind::Completed);
    assert!(cue.what.contains("finished"), "{}", cue.what);
}

/// A dead session's last painted menu cannot be answered, so it must not keep
/// asking. Otherwise the row carries a cue the operator has no way to clear.
#[test]
fn a_screen_cue_does_not_outlive_the_child_that_painted_it() {
    let mut row = row();
    row.attention = Some(HarnessAttention::new(
        AttentionKind::Approval,
        "codex is asking permission",
        NOW,
    ));
    row.state = PtyState::Exited { code: Some(0) };

    assert_eq!(row_cue(&row, NOW), None);
}

/// While it is alive, though, the screen is exactly where the cue comes from.
#[test]
fn a_live_session_keeps_the_cue_its_screen_produced() {
    let mut row = row();
    row.attention = Some(HarnessAttention::new(
        AttentionKind::Approval,
        "codex is asking permission",
        NOW,
    ));

    assert_eq!(
        row_cue(&row, NOW).expect("a cue").kind,
        AttentionKind::Approval
    );
}

/// A failure outranks whatever the frozen screen still shows.
#[test]
fn a_failure_outranks_the_screen() {
    let mut row = row();
    row.attention = Some(HarnessAttention::new(
        AttentionKind::Choice,
        "codex is waiting on a choice",
        NOW,
    ));
    row.state = PtyState::Failed;

    assert_eq!(
        row_cue(&row, NOW).expect("a cue").kind,
        AttentionKind::Failed
    );
}
