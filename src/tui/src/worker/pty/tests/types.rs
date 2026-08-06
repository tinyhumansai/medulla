//! The row model itself: what a [`SessionRow`] reports about a session, with no
//! child and no pseudo-terminal involved.

use medulla::protocol::HarnessProvider;

use crate::worker::pty::{PtyState, SessionControl, SessionOrigin, SessionRow};

/// A row for a session that is not running anywhere — only its fields matter.
fn row(provider: HarnessProvider, preset: Option<&str>) -> SessionRow {
    SessionRow {
        mcp_grant_session: None,
        id: "w_1".into(),
        label: "local".into(),
        provider,
        preset: preset.map(str::to_string),
        state: PtyState::Running,
        cwd: "/work".into(),
        branch: None,
        launch_root: None,
        launch_commit: None,
        launch_checkout_identity: None,
        session_id: None,
        thread_name: None,
        started_at: 1,
        last_output_at: 1,
        last_error: None,
        busy: false,
        control: SessionControl::Orchestrator,
        origin: SessionOrigin::Orchestrator,
        retained: false,
        name: None,
        attention: None,
    }
}

#[test]
fn the_harness_id_is_trimmed_at_the_source() {
    // It is a join key: callers match it against a declaration's `harness`,
    // which trims its own side. A preset id typed with a stray space would
    // otherwise compare unequal to the agent that declared it — and every
    // caller would have to remember to trim, which is the arrangement this
    // replaces.
    assert_eq!(
        row(HarnessProvider::Claude, Some(" my-preset ")).harness_id(),
        "my-preset"
    );
    // Untouched when there is nothing to trim.
    assert_eq!(
        row(HarnessProvider::Claude, Some("my-preset")).harness_id(),
        "my-preset"
    );
}

#[test]
fn a_blank_preset_is_no_preset_at_all() {
    // A whitespace-only preset is not an agent id, so the provider's wire name
    // is still the honest answer.
    assert_eq!(
        row(HarnessProvider::Claude, Some("   ")).harness_id(),
        "claude"
    );
    assert_eq!(row(HarnessProvider::Codex, None).harness_id(), "codex");
}
