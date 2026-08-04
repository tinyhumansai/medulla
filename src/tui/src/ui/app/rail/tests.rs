//! What the Agents rail assembles: declared agents with no traffic, sessions
//! grouped under the agent that owns them, and host rows only once there is a
//! second machine to tell apart.

use std::collections::HashMap;
use std::sync::Arc;

use medulla::config::LoadedConfig;
use medulla::protocol::HarnessProvider;
use medulla::runtime::mock::MockRuntime;
use medulla::runtime::{AgentDeclaration, Runtime};

use super::{RailRow, NEW_AGENT_LABEL};
use crate::ui::app::App;
use crate::worker::pty::PtyManager;

/// An Agents app on the mock runtime, hosting nothing.
pub(in crate::ui::app) fn app() -> App {
    let runtime: Arc<dyn Runtime> = Arc::new(MockRuntime::demo());
    let mut loaded = LoadedConfig::defaults("medulla.tui.json".into());
    loaded.config.link = Some(medulla::config::LinkConfig::default());
    App::new(runtime, loaded)
}

/// The same app with a live (but empty) local host attached, so the rail is
/// allowed to offer `+ New agent` and to list local sessions.
pub(in crate::ui::app) fn hosting_app() -> App {
    let mut app = app();
    app.set_local_harnesses(shell_harnesses(PtyManager::new()));
    app
}

/// A [`LocalHarnesses`](crate::ui::harness_pane::LocalHarnesses) whose "codex"
/// is `/bin/sh`, so opening one starts a real pty client and nothing else.
pub(in crate::ui::app) fn shell_harnesses(
    sessions: PtyManager,
) -> crate::ui::harness_pane::LocalHarnesses {
    let mut env = HashMap::new();
    if let Ok(path) = std::env::var("PATH") {
        env.insert("PATH".to_string(), path);
    }
    env.insert("TERM".to_string(), "xterm-256color".to_string());
    env.insert("TINYPLACE_CODEX_BIN".to_string(), "/bin/sh".to_string());
    crate::ui::harness_pane::LocalHarnesses {
        sessions,
        runtimes: Arc::new(std::sync::Mutex::new(Vec::new())),
        hub_address: "medulla-orchestrator".to_string(),
        env,
        workspace: "/".to_string(),
        providers: vec![HarnessProvider::Codex],
        custom_harnesses: Vec::new(),
        router: None,
        attribution: true,
    }
}

/// The agent rows, in rail order.
fn agent_ids(app: &App) -> Vec<String> {
    app.rail_rows()
        .into_iter()
        .filter_map(|row| match row {
            RailRow::Agent(agent) => Some(agent.agent_id),
            _ => None,
        })
        .collect()
}

#[test]
fn a_declared_agent_with_no_sessions_still_has_a_row() {
    // The point of sourcing the rail from declarations: an agent nothing has
    // been dispatched to folds no lane, so under the old taxonomy it had no row
    // at all — a targetable identity you could not see.
    let mut app = app();
    app.loaded.config.fleet.agent_declarations = vec![AgentDeclaration::new(
        "idle-agent",
        "",
        "claude",
        "/work/idle",
    )];

    let row = app
        .rail_rows()
        .into_iter()
        .find_map(|row| match row {
            RailRow::Agent(agent) if agent.agent_id == "idle-agent" => Some(agent),
            _ => None,
        })
        .expect("the declared agent has a row");

    assert!(row.lane_index.is_none(), "it has no traffic to fold");
    assert_eq!(row.harness(), Some("claude"));
    assert_eq!(row.workspace(), Some("/work/idle"));
}

#[test]
fn a_declaration_and_its_lane_are_one_row_not_two() {
    // Declaring an agent the fold already produced a lane for must not list it
    // twice: the id is the join, and the lane keeps its own live row.
    let mut app = app();
    let Some(existing) = agent_ids(&app).into_iter().next() else {
        return;
    };
    app.loaded.config.fleet.agent_declarations = vec![AgentDeclaration::new(
        existing.clone(),
        "",
        "claude",
        "/work",
    )];

    let ids = agent_ids(&app);
    assert_eq!(
        ids.iter().filter(|id| **id == existing).count(),
        1,
        "{ids:?} lists {existing} once"
    );
}

#[test]
fn every_session_row_sits_under_the_agent_that_owns_it() {
    // The rail's whole grouping rule, asserted structurally: walking the rows,
    // a session's agent is always whichever agent row it last passed.
    let app = app();
    let mut current: Option<String> = None;
    let mut checked = 0;
    for row in app.rail_rows() {
        match row {
            RailRow::Agent(agent) => current = Some(agent.agent_id),
            RailRow::Session(session) => {
                if let Some(agent_id) = &session.agent_id {
                    assert_eq!(
                        Some(agent_id),
                        current.as_ref(),
                        "a session is filed under the agent above it"
                    );
                    checked += 1;
                }
            }
            _ => {}
        }
    }
    assert!(checked > 0, "the demo fixture dispatches at least one task");
}

#[test]
fn a_dispatched_session_and_an_operator_session_are_the_same_row_type() {
    // §A0: the `── your harnesses ──` divider and the second row type are gone.
    // A task row and a PTY row are both `RailRow::Session`, so nothing on the
    // rail separates them into two groups.
    let mut app = hosting_app();
    app.loaded.config.fleet.agent_declarations =
        vec![AgentDeclaration::new("shell", "", "codex", "/")];
    let harnesses = app.local_harnesses().expect("hosting").clone();
    let choice = harnesses
        .choices()
        .into_iter()
        .find(|choice| choice.provider == HarnessProvider::Codex)
        .expect("codex is configured");
    let id = harnesses
        .open_unmanaged_named(&choice, "/", false, Some("debug login".into()))
        .expect("a /bin/sh session starts");

    let rows = app.rail_rows();
    let session = rows
        .iter()
        .find_map(|row| match row {
            RailRow::Session(session) if session.session_id() == Some(id.as_str()) => Some(session),
            _ => None,
        })
        .expect("the operator's session has a row");
    assert_eq!(session.agent_id.as_deref(), Some("shell"));
    assert_eq!(session.name(), Some("debug login"));
    assert!(session.origin().is_user());
    harnesses.sessions.shutdown();
}

#[test]
fn a_session_in_an_undeclared_directory_is_still_listed() {
    // Nothing declares `/`, so the session belongs to no agent — and a harness
    // that is running, costing tokens and invisible is exactly the failure the
    // old separate group existed to prevent.
    let app = hosting_app();
    let harnesses = app.local_harnesses().expect("hosting").clone();
    let choice = harnesses
        .choices()
        .into_iter()
        .find(|choice| choice.provider == HarnessProvider::Codex)
        .expect("codex is configured");
    let id = harnesses
        .open_unmanaged(&choice, "/", false)
        .expect("a /bin/sh session starts");

    let listed = app.rail_rows().into_iter().any(|row| match row {
        RailRow::Session(session) => {
            session.session_id() == Some(id.as_str()) && session.agent_id.is_none()
        }
        _ => false,
    });
    assert!(listed, "an unclaimed session is listed, not hidden");
    harnesses.sessions.shutdown();
}

#[test]
fn host_rows_appear_only_once_a_second_host_exists() {
    let mut app = app();
    app.loaded.config.fleet.agent_declarations =
        vec![AgentDeclaration::new("local-claude", "", "claude", "/work")];
    assert!(
        !app.rail_rows()
            .iter()
            .any(|row| matches!(row, RailRow::Host(_))),
        "one machine needs no host wrapper"
    );

    app.loaded
        .config
        .fleet
        .agent_declarations
        .push(AgentDeclaration::new(
            "studio-claude",
            "studio",
            "claude",
            "/work",
        ));
    let hosts: Vec<(String, bool)> = app
        .rail_rows()
        .into_iter()
        .filter_map(|row| match row {
            RailRow::Host(host) => Some((host.host_id, host.local)),
            _ => None,
        })
        .collect();
    assert!(
        hosts.iter().any(|(host_id, _)| host_id == "studio"),
        "the second machine gets a header: {hosts:?}"
    );
    assert!(
        hosts.len() >= 2,
        "so does this one, once there is a second: {hosts:?}"
    );
    let local = app.local_host_refs();
    assert_eq!(
        hosts.first().map(|(host_id, _)| host_id.as_str()),
        local.first().map(|host| host.id.as_str()),
        "this device leads, in the order the shared tree lists: {hosts:?}"
    );
}

#[test]
fn the_rail_and_the_hosts_tab_list_the_same_agents_under_the_same_hosts() {
    // The unification, asserted directly: both tabs render `host_rows`, so the
    // Agents rail cannot claim an agent the Hosts tab does not have, place it on
    // another host, or order them differently. The rail may hold *more* — a lane
    // for a backend-side agent this hub never advertised — and those are the
    // rows the tree does not cover, so they are excluded rather than asserted
    // away.
    let mut app = hosting_app();
    app.loaded.config.fleet.agent_declarations = vec![
        AgentDeclaration::new("api-claude", "", "claude", "/work/api"),
        AgentDeclaration::new("web-codex", "", "codex", "/work/web"),
        AgentDeclaration::new("studio-claude", "studio", "claude", "/work"),
    ];

    let expected: Vec<(String, String)> = app
        .host_tree()
        .into_iter()
        .flat_map(|host| {
            host.agents
                .into_iter()
                .map(move |agent| (host.id.clone(), agent.agent_id))
        })
        .collect();
    let known: Vec<String> = expected.iter().map(|(_, agent)| agent.clone()).collect();
    let railed: Vec<(String, String)> = app
        .rail_rows()
        .into_iter()
        .filter_map(|row| match row {
            RailRow::Agent(agent) if known.contains(&agent.agent_id) => {
                Some((agent.host_id, agent.agent_id))
            }
            _ => None,
        })
        .collect();

    assert!(!expected.is_empty(), "the fixture declares agents");
    assert_eq!(railed, expected, "one tree, two lenses");
}

#[test]
fn the_create_action_is_absent_on_a_device_that_hosts_nothing() {
    // A machine with no host cannot declare an agent on itself, so the action is
    // missing rather than present and refusing.
    assert!(
        !app().rail_rows().iter().any(RailRow::is_new_agent),
        "no host, no create action"
    );
    assert!(
        hosting_app().rail_rows().iter().any(RailRow::is_new_agent),
        "hosting, so {NEW_AGENT_LABEL} is offered"
    );
}

#[test]
fn an_agent_is_labelled_by_its_name_and_falls_back_to_its_id() {
    // The label, the harness and the workspace all come from the shared tree, so
    // an agent reads the same on both tabs. A row the tree does not cover has
    // only its id, and says nothing about the harness rather than guessing.
    let mut app = app();
    let mut declaration = AgentDeclaration::new("api-codex", "", "codex", "/work/api");
    app.loaded.config.fleet.agent_declarations = vec![declaration.clone()];

    let row = |app: &App| {
        app.rail_rows()
            .into_iter()
            .find_map(|row| match row {
                RailRow::Agent(agent) if agent.agent_id == "api-codex" => Some(agent),
                _ => None,
            })
            .expect("the declared agent has a row")
    };

    let declared = row(&app);
    assert_eq!(declared.label(), "api-codex", "no name means the id");
    assert_eq!(declared.harness(), Some("codex"));
    assert_eq!(declared.workspace(), Some("/work/api"));

    declaration.name = Some("   ".into());
    app.loaded.config.fleet.agent_declarations = vec![declaration.clone()];
    assert_eq!(row(&app).label(), "api-codex", "a blank name is not a name");

    declaration.name = Some("API".into());
    app.loaded.config.fleet.agent_declarations = vec![declaration];
    assert_eq!(row(&app).label(), "API");

    let bare = super::AgentRailRow {
        agent_id: "api-codex".into(),
        host_id: String::new(),
        agent: None,
        lane_index: None,
    };
    assert_eq!(bare.label(), "api-codex");
    assert_eq!(bare.harness(), None);
    assert_eq!(bare.workspace(), None);
}

#[test]
fn a_row_answers_for_the_agent_and_the_lane_behind_it() {
    let app = app();
    for row in app.rail_rows() {
        match &row {
            RailRow::Agent(agent) => {
                assert_eq!(row.agent_id(), Some(agent.agent_id.as_str()));
                assert_eq!(row.lane_index(), agent.lane_index);
                assert!(row.task().is_none(), "an agent is not a session");
            }
            RailRow::Session(session) => {
                assert_eq!(row.agent_id(), session.agent_id.as_deref());
                assert_eq!(row.lane_index(), session.lane_index);
            }
            // Hosts and the action row are about no agent and no lane.
            RailRow::Host(_) | RailRow::NewAgent => {
                assert_eq!(row.agent_id(), None);
                assert_eq!(row.lane_index(), None);
                assert_eq!(row.session_id(), None);
            }
            RailRow::Lane(lane) => assert_eq!(row.lane_index(), lane.lane_index()),
        }
    }
}

#[test]
fn only_the_rows_that_name_something_take_the_cursor() {
    let mut app = hosting_app();
    app.loaded.config.fleet.agent_declarations = vec![AgentDeclaration::new(
        "studio-claude",
        "studio",
        "claude",
        "/work",
    )];
    for row in app.rail_rows() {
        match row {
            RailRow::Host(_) => assert!(!row.selectable(), "a host header is a label"),
            RailRow::Agent(_) | RailRow::Session(_) | RailRow::NewAgent => {
                assert!(row.selectable())
            }
            RailRow::Lane(_) => {}
        }
    }
}
