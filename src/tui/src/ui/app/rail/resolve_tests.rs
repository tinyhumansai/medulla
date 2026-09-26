//! The session-to-agent resolution rule, exercised without an `App`.

use super::*;
use medulla::protocol::HarnessProvider;
use medulla::runtime::WorkspaceRef;

use crate::worker::pty::{PtyState, SessionControl, SessionOrigin};

fn session(provider: HarnessProvider, cwd: &str) -> SessionRow {
    SessionRow {
        mcp_grant_session: None,
        id: "w_1".into(),
        label: "local".into(),
        provider,
        preset: None,
        state: PtyState::Running,
        cwd: cwd.into(),
        checkout: Default::default(),
        launch_root: None,
        launch_commit: None,
        launch_checkout_identity: None,
        session_id: None,
        thread_name: None,
        started_at: 1,
        last_output_at: 1,
        last_error: None,
        busy: false,
        control: SessionControl::User,
        origin: SessionOrigin::User,
        retained: false,
        closed_by_request: false,
        name: None,
        attention: None,
        working: false,
    }
}

#[test]
fn a_session_matches_the_declaration_of_its_harness_and_directory() {
    let declarations = vec![
        AgentDeclaration::new("api-claude", "host", "claude", "/work/api"),
        AgentDeclaration::new("web-claude", "host", "claude", "/work/web"),
    ];
    let matched = agent_for_session(
        &declarations,
        &session(HarnessProvider::Claude, "/work/web"),
    )
    .expect("the web checkout is declared");
    assert_eq!(matched.agent_id, "web-claude");
}

#[test]
fn a_trailing_separator_is_not_a_different_directory() {
    let mut declaration = AgentDeclaration::new("api-claude", "host", "claude", "");
    declaration.workspace = WorkspaceRef::checkout("/work/api/");
    let declarations = vec![declaration];
    assert!(agent_for_session(
        &declarations,
        &session(HarnessProvider::Claude, "/work/api")
    )
    .is_some());
}

#[test]
fn a_different_harness_in_the_same_directory_is_a_different_agent() {
    let declarations = vec![AgentDeclaration::new(
        "api-codex",
        "host",
        "codex",
        "/work/api",
    )];
    assert!(
        agent_for_session(
            &declarations,
            &session(HarnessProvider::Claude, "/work/api")
        )
        .is_none(),
        "claude in the codex agent's directory is not that agent"
    );
}

#[test]
fn a_preset_backed_session_belongs_to_the_agent_declared_for_that_preset() {
    // A custom preset is its own agent — its own model, endpoint and
    // environment — so a declaration records the *preset's* id. Matching on
    // the CLI underneath it compared `claude` against `deepseek`, and every
    // session an operator started from a preset was listed as belonging to
    // no agent at all.
    let declarations = vec![AgentDeclaration::new(
        "api-deepseek",
        "host",
        "deepseek",
        "/work/api",
    )];
    let mut row = session(HarnessProvider::Claude, "/work/api");
    row.preset = Some("deepseek".into());

    let matched = agent_for_session(&declarations, &row).expect("its own agent claims it");
    assert_eq!(matched.agent_id, "api-deepseek");
    assert_eq!(row.harness_id(), "deepseek");
}

#[test]
fn a_preset_is_not_the_base_cli_declared_in_the_same_directory() {
    // The other direction of the same rule: an agent declared as plain
    // `claude` is not the one a `deepseek` preset session belongs to, even
    // though both run the claude binary in that folder.
    let declarations = vec![AgentDeclaration::new(
        "api-claude",
        "host",
        "claude",
        "/work/api",
    )];
    let mut row = session(HarnessProvider::Claude, "/work/api");
    row.preset = Some("deepseek".into());
    assert!(agent_for_session(&declarations, &row).is_none());

    // And a native session still resolves by the provider's wire name: a
    // blank preset is no preset.
    let mut native = session(HarnessProvider::Claude, "/work/api");
    native.preset = Some("   ".into());
    assert_eq!(native.harness_id(), "claude");
    assert!(agent_for_session(&declarations, &native).is_some());
}

#[test]
fn an_undeclared_directory_resolves_to_no_agent() {
    let declarations = vec![AgentDeclaration::new(
        "api-claude",
        "host",
        "claude",
        "/work/api",
    )];
    assert!(agent_for_session(
        &declarations,
        &session(HarnessProvider::Claude, "/elsewhere")
    )
    .is_none());
}
