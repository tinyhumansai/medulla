//! Unit tests for the Hosts page's tree, its cursor, and its edits.

use std::sync::Arc;

use medulla::config::LoadedConfig;
use medulla::runtime::mock::MockRuntime;
use medulla::runtime::{AgentDeclaration, WorkerInfo};
use medulla::ui::hosts::HostKind;

use super::super::types::{App, Cmd};

/// A roster entry at `address`, with no capability probe.
fn worker(id: &str, address: &str) -> WorkerInfo {
    WorkerInfo {
        id: id.into(),
        address: address.into(),
        handle: None,
        label: None,
        harness: Some("claude".into()),
        workspace: Some("/w/checkout".into()),
        peer_id: None,
        cpu_cores: None,
        memory_total_bytes: None,
        memory_available_bytes: None,
        ip_address: None,
        selected: false,
        roles: Vec::new(),
        budgets: Vec::new(),
        readiness: Vec::new(),
    }
}

/// An app over `workers` and `declarations`, writing to a real config file so
/// the persistence path is exercised rather than stubbed.
fn app_with(
    workers: Vec<WorkerInfo>,
    declarations: Vec<AgentDeclaration>,
) -> (App, tempfile::TempDir) {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("medulla.tui.json");
    std::fs::write(&path, "{}").unwrap();
    let runtime = MockRuntime::empty();
    runtime.set_workers(workers);
    let mut loaded = LoadedConfig::defaults(path.to_string_lossy().into_owned());
    loaded.config.fleet.agent_declarations = declarations;
    let mut app = App::new(Arc::new(runtime), loaded);
    app.set_config_path(path);
    (app, dir)
}

/// Move the cursor to the row `steps` below the top of the list.
fn cursor_to(app: &mut App, steps: usize) {
    app.focus_routing_subpage("Hosts");
    for _ in 0..steps {
        let _ = app.on_event(crossterm::event::Event::Key(
            crossterm::event::KeyEvent::new(
                crossterm::event::KeyCode::Down,
                crossterm::event::KeyModifiers::NONE,
            ),
        ));
    }
}

#[test]
fn the_local_host_leads_the_tree_with_its_agents_under_it() {
    let (app, _dir) = app_with(
        vec![
            worker("medulla-claude", "this-device"),
            worker("peer", "7Kx"),
        ],
        vec![AgentDeclaration::new(
            "medulla-claude",
            "this-device",
            "claude",
            "/w/medulla",
        )],
    );

    let tree = app.host_tree();
    assert_eq!(tree.len(), 2, "the local host and one remote: {tree:?}");
    assert_eq!(tree[0].id, "this-device");
    assert_eq!(tree[0].kind, HostKind::Local);
    assert_eq!(tree[0].agents.len(), 1);
    assert_eq!(tree[1].id, "7Kx");
    assert_eq!(tree[1].kind, HostKind::Remote);
    // Four rows: two headers, two agents.
    assert_eq!(app.hosts_row_count(), 4);
}

#[test]
fn the_cursor_walks_hosts_and_the_agents_under_them() {
    let (mut app, _dir) = app_with(vec![worker("peer", "7Kx")], Vec::new());
    cursor_to(&mut app, 0);

    // Row 0 is the local host header, and it is where a new agent may go.
    assert!(app.hosts_cursor_on_host());
    assert!(app.selected_host_is_local());
    assert!(app.selected_host_agent().is_none());

    // Row 1 is the remote host, row 2 the agent the roster knows on it.
    cursor_to(&mut app, 1);
    assert!(app.hosts_cursor_on_host());
    assert!(!app.selected_host_is_local());
    cursor_to(&mut app, 1);
    assert_eq!(
        app.selected_host_agent().map(|agent| agent.agent_id),
        Some("peer".to_string())
    );
}

#[test]
fn a_role_toggle_is_written_to_the_declaration_and_survives_a_reload() {
    let declaration =
        AgentDeclaration::new("medulla-claude", "this-device", "claude", "/w/medulla");
    let (mut app, dir) = app_with(
        vec![worker("medulla-claude", "this-device")],
        vec![declaration],
    );
    cursor_to(&mut app, 1); // the local host's only agent

    let role = app
        .agent_templates()
        .first()
        .expect("built-in roles")
        .id
        .clone();
    let cmd = app.toggle_selected_agent_role(&role);

    // The live roster moves too, so the orchestrator starts routing the role
    // here without waiting for a restart.
    match cmd {
        Some(Cmd::WorkerOp(medulla::runtime::WorkerOp::SetRoles { id, roles })) => {
            assert_eq!(id, "medulla-claude");
            assert_eq!(roles, vec![role.clone()]);
        }
        other => panic!("expected a SetRoles op, got {other:?}"),
    }
    // And — the point of the whole exercise — the file has it.
    let written = medulla::config::load_agent_declarations(&dir.path().join("medulla.tui.json"));
    assert_eq!(written.len(), 1);
    assert_eq!(written[0].roles, vec![role.clone()]);
    assert_eq!(written[0].harness, "claude");

    // Toggling back sends an empty list and clears it on disk, so the checkbox
    // is not one-way.
    let cmd = app.toggle_selected_agent_role(&role);
    match cmd {
        Some(Cmd::WorkerOp(medulla::runtime::WorkerOp::SetRoles { roles, .. })) => {
            assert!(roles.is_empty(), "removal sends an empty list: {roles:?}")
        }
        other => panic!("expected a SetRoles op, got {other:?}"),
    }
    let written = medulla::config::load_agent_declarations(&dir.path().join("medulla.tui.json"));
    assert!(written[0].roles.is_empty());
}

#[test]
fn giving_a_seeded_agent_a_role_declares_it() {
    // The migration case: the roster has an agent nobody wrote down. A role must
    // still persist, which means the toggle declares it from what the roster
    // reports rather than refusing.
    let (mut app, dir) = app_with(vec![worker("this-device", "this-device")], Vec::new());
    cursor_to(&mut app, 1);

    let role = app
        .agent_templates()
        .first()
        .expect("built-in roles")
        .id
        .clone();
    assert!(app.toggle_selected_agent_role(&role).is_some());

    let written = medulla::config::load_agent_declarations(&dir.path().join("medulla.tui.json"));
    assert_eq!(written.len(), 1, "the seed became a declaration");
    assert_eq!(written[0].agent_id, "this-device");
    assert_eq!(written[0].host_id, "this-device");
    assert_eq!(written[0].harness, "claude");
    assert_eq!(written[0].workspace.path, "/w/checkout");
    assert_eq!(written[0].roles, vec![role]);
    // The in-memory config agrees with the file, so the row redraws assigned.
    assert_eq!(app.loaded.config.fleet.agent_declarations.len(), 1);
}

#[test]
fn the_agent_preview_never_draws_past_the_rows_it_was_given() {
    // The role list is windowed to whatever the fixed agent details left over.
    // A zero budget used to still force one checkbox through, so the block ran
    // one row past the bottom of the pane on a short terminal — and the row that
    // fell off was the one carrying the role cursor.
    let (mut app, _dir) = app_with(
        vec![worker("medulla-claude", "this-device")],
        vec![AgentDeclaration::new(
            "medulla-claude",
            "this-device",
            "claude",
            "/w/medulla",
        )],
    );
    cursor_to(&mut app, 1);
    let (tree, rows, selected) = app.hosts_view();
    let row = rows[selected];
    assert!(row.agent.is_some(), "the cursor is on the agent row");

    // The fixed identity block is the agent, so it is always drawn; what the
    // budget governs is the role list hung under it.
    let details = app.preview_height_within(&tree, row, 0);
    assert!(details > 1, "the identity block is several rows: {details}");
    for budget in 0..details {
        assert_eq!(
            app.preview_height_within(&tree, row, budget),
            details,
            "a {budget}-row pane has no room for roles at all"
        );
    }
    // One row to spare buys the summary — the sentence saying what the agent is
    // offered for — rather than one checkbox out of a dozen, which reads as the
    // whole list.
    assert_eq!(
        app.preview_height_within(&tree, row, details + 1),
        details + 1
    );
    for budget in details..=details + 12 {
        assert!(
            app.preview_height_within(&tree, row, budget) <= budget,
            "a {budget}-row pane drew {} rows",
            app.preview_height_within(&tree, row, budget)
        );
    }
}

#[test]
fn a_remote_agent_is_read_only() {
    let (mut app, dir) = app_with(vec![worker("peer", "7Kx")], Vec::new());
    cursor_to(&mut app, 2); // local header, remote header, remote agent

    let agent = app.selected_host_agent().expect("a remote agent row");
    assert!(!agent.editable);

    let role = app
        .agent_templates()
        .first()
        .expect("built-in roles")
        .id
        .clone();
    assert!(
        app.toggle_selected_agent_role(&role).is_none(),
        "a remote agent's roles are that machine's to assign"
    );
    assert!(
        app.status().contains("declared on"),
        "and the refusal says why: {}",
        app.status()
    );
    let written = medulla::config::load_agent_declarations(&dir.path().join("medulla.tui.json"));
    assert!(written.is_empty(), "nothing is declared for a remote host");
}

#[test]
fn a_failed_write_changes_nothing_at_all() {
    // A UI showing a role the file does not have is worse than one that refused:
    // the operator would believe the assignment survived the restart it will not.
    let (mut app, dir) = app_with(
        vec![worker("medulla-claude", "this-device")],
        vec![AgentDeclaration::new(
            "medulla-claude",
            "this-device",
            "claude",
            "/w/medulla",
        )],
    );
    // A directory cannot be written as a config file.
    app.set_config_path(dir.path().to_path_buf());
    cursor_to(&mut app, 1);

    let role = app
        .agent_templates()
        .first()
        .expect("built-in roles")
        .id
        .clone();
    assert!(app.toggle_selected_agent_role(&role).is_none());
    assert!(
        app.status().starts_with("Roles were not saved"),
        "status: {}",
        app.status()
    );
    assert!(
        app.loaded.config.fleet.agent_declarations[0]
            .roles
            .is_empty(),
        "the in-memory list must not drift from the file"
    );
}

/// The same app with no config file at all, which is the "this run only" path.
fn app_without_config(workers: Vec<WorkerInfo>, declarations: Vec<AgentDeclaration>) -> App {
    let runtime = MockRuntime::empty();
    runtime.set_workers(workers);
    let mut loaded = LoadedConfig::defaults("medulla.tui.json".into());
    loaded.config.fleet.agent_declarations = declarations;
    App::new(Arc::new(runtime), loaded)
}

#[test]
fn with_no_config_file_an_edit_lasts_the_run_and_says_so() {
    // Nowhere to write is not a refusal — the roster is in front of the
    // operator and has to stay editable — but it must not read as saved
    // either. So the edit lands in *both* places for this run: the roster op
    // the caller sends, and the declaration list the tree redraws from. Only
    // updating the roster left the row showing the roles it had before.
    let mut app = app_without_config(
        vec![worker("medulla-claude", "this-device")],
        vec![AgentDeclaration::new(
            "medulla-claude",
            "this-device",
            "claude",
            "/w/medulla",
        )],
    );
    cursor_to(&mut app, 1);

    let role = app
        .agent_templates()
        .first()
        .expect("built-in roles")
        .id
        .clone();
    assert!(app.toggle_selected_agent_role(&role).is_some());
    assert!(
        app.status().contains("this run only"),
        "the operator is told how long it lasts: {}",
        app.status()
    );
    assert_eq!(
        app.loaded.config.fleet.agent_declarations[0].roles,
        vec![role.clone()],
        "the declaration the tree reads from moved too"
    );
    assert_eq!(
        app.selected_host_agent().map(|agent| agent.roles),
        Some(vec![role.clone()]),
        "so the row redraws assigned rather than reverting"
    );

    // And the toggle is not one-way: the second press sees the role it just
    // wrote and takes it back off.
    assert!(app.toggle_selected_agent_role(&role).is_some());
    assert!(app.loaded.config.fleet.agent_declarations[0]
        .roles
        .is_empty());
}

#[test]
fn with_no_config_file_a_rename_lasts_the_run_and_says_so() {
    // The roster label has already changed by the time the persist runs, so a
    // silent return would leave the operator reading a name that disappears at
    // the next launch with nothing having said so.
    let mut app = app_without_config(
        Vec::new(),
        vec![AgentDeclaration::new(
            "api-codex",
            "this-device",
            "codex",
            "/w/api",
        )],
    );

    app.persist_agent_name("api-codex", "Backend");

    assert!(
        app.status().contains("this run only"),
        "status: {}",
        app.status()
    );
    assert_eq!(
        app.loaded.config.fleet.agent_declarations[0]
            .name
            .as_deref(),
        Some("Backend"),
        "the name is the run's, in the same place the saved one would be"
    );
}

#[test]
fn with_no_config_file_undeclaring_lasts_the_run_and_says_so() {
    // One rule for the whole path: removing is an edit like the others, so it
    // applies for this run rather than being refused — and the status is what
    // stops the agent's return at the next launch being a surprise.
    let mut app = app_without_config(
        Vec::new(),
        vec![AgentDeclaration::new(
            "api-codex",
            "this-device",
            "codex",
            "/w/api",
        )],
    );
    cursor_to(&mut app, 1);

    assert!(app.undeclare_selected_agent());
    assert!(app.loaded.config.fleet.agent_declarations.is_empty());
    assert!(
        app.status().contains("this run only"),
        "status: {}",
        app.status()
    );
}

#[test]
fn an_agent_with_no_workspace_cannot_be_declared_by_a_role_toggle() {
    // An agent is `harness × workspace`. Seeding one with no directory writes a
    // declaration no session can be opened from — `open_new_session` refuses it
    // with "declares no workspace" — so the role toggle says so up front rather
    // than saving a row the operator then cannot use.
    let mut bare = worker("mystery", "this-device");
    bare.workspace = None;
    let (mut app, dir) = app_with(vec![bare], Vec::new());
    cursor_to(&mut app, 1);

    let role = app
        .agent_templates()
        .first()
        .expect("built-in roles")
        .id
        .clone();
    assert!(app.toggle_selected_agent_role(&role).is_none());
    assert!(app.status().contains("no workspace"), "{}", app.status());
    assert!(
        medulla::config::load_agent_declarations(&dir.path().join("medulla.tui.json")).is_empty()
    );
}

#[test]
fn an_agent_with_no_harness_cannot_be_declared_by_a_role_toggle() {
    // Declaring an agent with no harness would advertise a placement that cannot
    // run, so the toggle says so instead of inventing one.
    let mut bare = worker("mystery", "this-device");
    bare.harness = None;
    let (mut app, dir) = app_with(vec![bare], Vec::new());
    cursor_to(&mut app, 1);

    let role = app
        .agent_templates()
        .first()
        .expect("built-in roles")
        .id
        .clone();
    assert!(app.toggle_selected_agent_role(&role).is_none());
    assert!(app.status().contains("no harness"), "{}", app.status());
    assert!(
        medulla::config::load_agent_declarations(&dir.path().join("medulla.tui.json")).is_empty()
    );
}

#[test]
fn undeclaring_an_agent_removes_it_from_the_file_only() {
    let (mut app, dir) = app_with(
        Vec::new(),
        vec![AgentDeclaration::new(
            "api-codex",
            "this-device",
            "codex",
            "/w/api",
        )],
    );
    cursor_to(&mut app, 1);

    assert!(app.undeclare_selected_agent());
    assert!(
        medulla::config::load_agent_declarations(&dir.path().join("medulla.tui.json")).is_empty()
    );
    assert!(app.loaded.config.fleet.agent_declarations.is_empty());
    // An agent the roster owns but this machine never declared is not ours to
    // undeclare — that path removes the roster entry instead.
    let (mut remote, _dir) = app_with(vec![worker("peer", "7Kx")], Vec::new());
    cursor_to(&mut remote, 2);
    assert!(!remote.undeclare_selected_agent());
}

#[test]
fn a_host_this_device_does_not_serve_drops_out_unless_it_still_holds_something() {
    let (mut app, _dir) = app_with(Vec::new(), Vec::new());
    assert_eq!(app.host_tree().len(), 1, "hosting is on by default");

    app.loaded.config.host.enabled = false;
    assert!(
        app.host_tree().is_empty(),
        "nothing declared, nothing running, not hosting"
    );

    app.loaded.config.fleet.agent_declarations = vec![AgentDeclaration::new(
        "api-codex",
        "this-device",
        "codex",
        "/w/api",
    )];
    assert_eq!(
        app.host_tree().len(),
        1,
        "declared agents keep their host listed"
    );
}

/// Press `d` on whatever the cursor is on, and return the fleet mutation it
/// asked for.
fn press_delete(app: &mut App) -> Option<Cmd> {
    app.on_event(crossterm::event::Event::Key(
        crossterm::event::KeyEvent::new(
            crossterm::event::KeyCode::Char('d'),
            crossterm::event::KeyModifiers::NONE,
        ),
    ))
}

/// The ids a fleet mutation removes, in order.
fn removed_ids(cmd: Option<Cmd>) -> Vec<String> {
    match cmd {
        Some(Cmd::WorkerOp(medulla::runtime::WorkerOp::Remove { id })) => vec![id],
        Some(Cmd::WorkerOps(ops)) => ops
            .into_iter()
            .map(|op| match op {
                medulla::runtime::WorkerOp::Remove { id } => id,
                other => panic!("expected only removals, got {other:?}"),
            })
            .collect(),
        other => panic!("expected a removal, got {other:?}"),
    }
}

#[test]
fn deleting_a_host_takes_every_agent_on_it_not_just_the_one_that_probed_it() {
    // Two agents on one remote machine. Before this was pointer-aware, `d` on
    // the host row resolved to the host's `detail_worker` — a single entry —
    // and removed that one agent while leaving the host and its sibling.
    let (mut app, _dir) = app_with(
        vec![worker("peer-claude", "7Kx"), worker("peer-codex", "7Kx")],
        Vec::new(),
    );
    // Row 0 is the local host; row 1 is the remote host header.
    cursor_to(&mut app, 1);
    assert!(app.hosts_cursor_on_host(), "the cursor is on the host row");

    let removed = removed_ids(press_delete(&mut app));
    assert_eq!(
        removed,
        vec!["peer-claude".to_string(), "peer-codex".to_string()],
        "both agents on the host go, not only the one that probed it"
    );
}

#[test]
fn deleting_an_agent_leaves_its_host_and_its_siblings_standing() {
    let (mut app, _dir) = app_with(
        vec![worker("peer-claude", "7Kx"), worker("peer-codex", "7Kx")],
        Vec::new(),
    );
    // Row 2 is the first agent under the remote host.
    cursor_to(&mut app, 2);
    assert_eq!(
        app.selected_host_agent().map(|agent| agent.agent_id),
        Some("peer-claude".to_string())
    );

    assert_eq!(
        removed_ids(press_delete(&mut app)),
        vec!["peer-claude".to_string()],
        "only the agent under the cursor"
    );
}

#[test]
fn deleting_a_declared_agent_by_its_host_row_undeclares_it_too() {
    // A declaration is what re-creates an agent at the next launch, so removing
    // the host has to drop it as well or the removal does not survive a restart.
    let (mut app, _dir) = app_with(
        vec![worker("peer-codex", "7Kx")],
        vec![AgentDeclaration::new(
            "peer-codex",
            "7Kx",
            "codex",
            "/w/api",
        )],
    );
    cursor_to(&mut app, 1);
    assert!(app.hosts_cursor_on_host());

    assert_eq!(
        removed_ids(press_delete(&mut app)),
        vec!["peer-codex".to_string()]
    );
    // Asserted on the list the app reads, not on the file: `app_with` seeds
    // declarations in memory only, so a file assertion here would pass whether
    // or not the removal happened.
    assert!(
        app.loaded.config.fleet.agent_declarations.is_empty(),
        "the declaration went with the host"
    );
}

#[test]
fn a_host_this_device_runs_is_not_removable() {
    // It is declared by `[host]` rather than by a removable entry, so it would
    // be back at the next launch — and a running host with no declarations
    // seeds one agent per CLI on PATH, so emptying it is exactly what brings
    // them back. Reporting a removal that does not last is worse than refusing.
    let (mut app, _dir) = app_with(
        vec![worker("medulla-claude", "this-device")],
        vec![AgentDeclaration::new(
            "medulla-claude",
            "this-device",
            "claude",
            "/w/medulla",
        )],
    );
    cursor_to(&mut app, 0);
    assert!(app.hosts_cursor_on_host());
    assert!(app.selected_host_is_local());

    assert!(
        press_delete(&mut app).is_none(),
        "the local host asks for no fleet mutation"
    );
    assert_eq!(
        app.loaded.config.fleet.agent_declarations.len(),
        1,
        "and it takes nothing down with it"
    );
}

#[test]
fn deleting_a_seeded_agent_declares_the_rest_so_the_removal_survives() {
    // The state every fresh install is in: agents in the roster, nothing in
    // `[fleet].agentDeclarations`. Those rows are seeded from the CLIs on PATH,
    // so there is no declaration to shorten — and removing only the roster entry
    // would let the seed put it back at the next start.
    let (mut app, _dir) = app_with(
        vec![
            worker("medulla-claude", "this-device"),
            worker("medulla-codex", "this-device"),
        ],
        Vec::new(),
    );
    cursor_to(&mut app, 1);
    let target = app
        .selected_host_agent()
        .expect("row 1 is an agent")
        .agent_id;
    assert!(
        app.loaded.config.fleet.agent_declarations.is_empty(),
        "nothing is declared yet"
    );

    let _ = press_delete(&mut app);

    let declared: Vec<String> = app
        .loaded
        .config
        .fleet
        .agent_declarations
        .iter()
        .map(|declaration| declaration.agent_id.clone())
        .collect();
    assert!(
        !declared.contains(&target),
        "the removed agent is not declared: {declared:?}"
    );
    assert!(
        !declared.is_empty(),
        "and its siblings now are, so the list stops being seeded: {declared:?}"
    );
}

#[test]
fn adopting_seeds_does_not_rewrite_the_agents_already_declared() {
    // A host can hold both kinds of row at once. The declared one carries fields
    // the projection never sees — the operator's name for it, and any strategy
    // other than the default — so rebuilding it from the row would drop them
    // while looking like it had preserved everything.
    let mut named = AgentDeclaration::new("medulla-claude", "this-device", "claude", "/w/medulla");
    named.name = Some("build box".into());
    let (mut app, _dir) = app_with(
        vec![
            worker("medulla-claude", "this-device"),
            worker("medulla-codex", "this-device"),
        ],
        vec![named],
    );
    // Row 2 is the seeded sibling: undeclared, so removing it adopts the rest.
    cursor_to(&mut app, 2);
    let target = app
        .selected_host_agent()
        .expect("row 2 is an agent")
        .agent_id;
    assert_ne!(
        target, "medulla-claude",
        "the seeded row, not the declared one"
    );

    let _ = press_delete(&mut app);

    let kept = app
        .loaded
        .config
        .fleet
        .agent_declarations
        .iter()
        .find(|declaration| declaration.agent_id == "medulla-claude")
        .expect("the declared survivor is still declared");
    assert_eq!(
        kept.name.as_deref(),
        Some("build box"),
        "and kept the name the projection does not carry"
    );
}

#[test]
fn a_named_agent_is_listed_by_its_name_without_losing_its_id() {
    // The declare flow ends in a name prompt, and the name reached the config —
    // but the row rendered the agent id, so naming one looked like it had done
    // nothing. The id has to stay: it is what a dispatch targets and what every
    // status line about this agent says.
    let mut named = AgentDeclaration::new("medulla-claude", "this-device", "claude", "/w/medulla");
    named.name = Some("build box".into());
    let (app, _dir) = app_with(vec![worker("medulla-claude", "this-device")], vec![named]);

    let tree = app.host_tree();
    let agent = &tree[0].agents[0];
    assert_eq!(agent.label, "build box", "the projection carries the name");
    assert_eq!(
        agent.agent_id, "medulla-claude",
        "and the id it is dispatched by"
    );
}
