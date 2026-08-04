//! The two writes the Agents tab makes: declaring an agent, and opening a
//! session under one.

use medulla::config::load_agent_declarations;
use medulla::protocol::HarnessProvider;
use medulla::runtime::AgentDeclaration;

use super::rail::tests::{hosting_app, shell_harnesses};
use super::rail::RailRow;
use super::types::{PickerPurpose, PromptKind};
use crate::worker::pty::PtyManager;

/// A hosting app whose config lives in `dir`, so a declaration can be read back.
fn app_with_config(dir: &std::path::Path) -> super::types::App {
    let mut app = hosting_app();
    app.set_config_path(dir.join("config.toml"));
    app
}

#[test]
fn declaring_an_agent_writes_it_and_shows_it_on_the_rail() {
    let dir = tempfile::tempdir().expect("a temp dir");
    let mut app = app_with_config(dir.path());

    app.declare_new_agent("codex", "/work/api", "API");

    let written = load_agent_declarations(&dir.path().join("config.toml"));
    assert_eq!(written.len(), 1, "one agent is on disk: {written:?}");
    let declaration = &written[0];
    // The id is minted from the folder, not typed: it is the dispatch target.
    assert_eq!(declaration.agent_id, "api-codex");
    assert_eq!(declaration.harness, "codex");
    assert_eq!(declaration.workspace.path(), Some("/work/api"));
    assert_eq!(declaration.name.as_deref(), Some("API"));
    // `checkout` is the only strategy a picker may offer in v1, so one session
    // writes at a time.
    assert_eq!(declaration.max_sessions(), 1);

    // The written list is adopted in memory too, so the very next frame has the
    // row rather than waiting for a restart.
    assert!(
        app.rail_rows()
            .iter()
            .any(|row| matches!(row, RailRow::Agent(agent) if agent.agent_id == "api-codex")),
        "the declared agent is on the rail"
    );
}

#[test]
fn a_blank_name_keeps_the_minted_id_and_a_second_agent_does_not_collide() {
    let dir = tempfile::tempdir().expect("a temp dir");
    let mut app = app_with_config(dir.path());

    app.declare_new_agent("codex", "/work/api", "   ");
    app.declare_new_agent("codex", "/other/api", "");

    let written = load_agent_declarations(&dir.path().join("config.toml"));
    let ids: Vec<&str> = written
        .iter()
        .map(|declaration| declaration.agent_id.as_str())
        .collect();
    assert_eq!(ids, vec!["api-codex", "api-codex-2"], "ids never collide");
    assert!(
        written.iter().all(|declaration| declaration.name.is_none()),
        "blank is not a name"
    );
}

#[test]
fn a_failed_write_leaves_the_rail_showing_what_is_on_disk() {
    // No config path at all: nothing can be written, so nothing is adopted —
    // the alternative is a rail showing an agent that will not survive a
    // restart.
    let mut app = hosting_app();
    app.declare_new_agent("codex", "/work/api", "API");
    assert!(app.agent_declarations().is_empty());
    assert!(app.status().contains("No config file"), "{}", app.status());
}

#[test]
fn the_new_agent_row_opens_the_picker_in_declare_mode() {
    let mut app = hosting_app();
    let index = app
        .rail_rows()
        .iter()
        .position(RailRow::is_new_agent)
        .expect("the create action is on the rail");
    app.agent_index = index;
    assert!(app.on_new_agent_row());

    app.open_new_agent_picker();
    let picker = app.harness_picker.as_ref().expect("the picker opened");
    assert_eq!(picker.purpose, PickerPurpose::DeclareAgent);
}

#[test]
fn a_session_started_somewhere_undeclared_offers_an_agent_for_it() {
    let mut app = hosting_app();
    app.offer_agent_declaration("codex", "/work/loose");
    assert!(app.prompt_state().is_some(), "the offer is a name prompt");
    let kind = app.prompt.as_ref().map(|prompt| &prompt.kind);
    assert!(
        matches!(kind, Some(PromptKind::AgentName { harness, workspace })
            if harness == "codex" && workspace == "/work/loose"),
        "the offer carries the pair it would declare"
    );
}

#[test]
fn a_directory_that_is_already_declared_is_not_offered_again() {
    let mut app = hosting_app();
    app.loaded.config.fleet.agent_declarations =
        vec![AgentDeclaration::new("api", "", "codex", "/work/api")];
    app.offer_agent_declaration("codex", "/work/api/");
    assert!(app.prompt_state().is_none(), "nothing left to declare");
}

#[test]
fn a_new_session_under_an_agent_asks_for_a_name_first() {
    let mut app = hosting_app();
    app.loaded.config.fleet.agent_declarations =
        vec![AgentDeclaration::new("shell", "", "codex", "/")];

    app.open_new_session("shell");

    let kind = app.prompt.as_ref().map(|prompt| &prompt.kind);
    assert!(
        matches!(kind, Some(PromptKind::SessionName { agent_id, managed })
            if agent_id == "shell" && !managed),
        "a session a person spins up is theirs at birth"
    );
}

#[test]
fn a_named_session_opens_in_the_agents_own_harness_and_workspace() {
    let sessions = PtyManager::new();
    let mut app = hosting_app();
    app.set_local_harnesses(shell_harnesses(sessions.clone()));
    app.loaded.config.fleet.agent_declarations =
        vec![AgentDeclaration::new("shell", "", "codex", "/")];

    app.start_agent_session("shell", "debug login", false);

    let rows = sessions.rows();
    let row = rows.first().expect("a session opened").clone();
    assert_eq!(row.name.as_deref(), Some("debug login"));
    assert_eq!(row.provider, HarnessProvider::Codex, "the agent's harness");
    assert_eq!(row.cwd, "/", "the agent's workspace, not the caller's");
    assert!(row.origin.is_user(), "the operator started it");
    // The cursor moves onto the new row: a session that appears below the fold
    // with the pane unchanged reads as "nothing happened".
    assert_eq!(
        app.rail_rows()
            .get(app.agent_index())
            .and_then(RailRow::session_id),
        Some(row.id.as_str())
    );
    sessions.shutdown();
}

#[test]
fn opening_a_session_for_an_undeclared_agent_says_so() {
    let mut app = hosting_app();
    app.open_new_session("nobody");
    assert!(app.status().contains("nobody"), "{}", app.status());
    assert!(app.prompt_state().is_none());
}
