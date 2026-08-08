//! What the Sessions rail assembles: declared agents with no traffic, sessions
//! grouped under the agent that owns them, and host rows only once there is a
//! second machine to tell apart.

use std::collections::HashMap;
use std::sync::Arc;

use medulla::config::LoadedConfig;
use medulla::protocol::HarnessProvider;
use medulla::runtime::mock::MockRuntime;
use medulla::runtime::{AgentDeclaration, Runtime, WorkerInfo};

use super::{AgentGroup, AgentRailRow, GroupRailRow, RailRow, NEW_SESSION_LABEL};
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
    app.set_local_sessions(shell_harnesses(PtyManager::new()));
    app
}

/// A [`LocalSessions`](crate::ui::harness_pane::LocalSessions) whose "codex"
/// is `/bin/sh`, so opening one starts a real pty client and nothing else.
pub(in crate::ui::app) fn shell_harnesses(
    sessions: PtyManager,
) -> crate::ui::harness_pane::LocalSessions {
    let mut env = HashMap::new();
    if let Ok(path) = std::env::var("PATH") {
        env.insert("PATH".to_string(), path);
    }
    env.insert("TERM".to_string(), "xterm-256color".to_string());
    env.insert("TINYPLACE_CODEX_BIN".to_string(), "/bin/sh".to_string());
    crate::ui::harness_pane::LocalSessions {
        sessions,
        runtimes: Arc::new(std::sync::Mutex::new(Vec::new())),
        hub_address: "medulla-orchestrator".to_string(),
        env,
        workspace: "/".to_string(),
        providers: vec![HarnessProvider::Codex],
        custom_harnesses: Vec::new(),
        router: None,
        attribution: true,
        hooks: medulla::harness_hooks::HooksConfig::default(),
        log: None,
    }
}

/// A minimal live pty row, for the rail rules that only look at its identity.
pub(in crate::ui::app) fn stub_session(id: &str) -> crate::worker::pty::SessionRow {
    crate::worker::pty::SessionRow {
        id: id.to_string(),
        label: id.to_string(),
        provider: HarnessProvider::Codex,
        preset: None,
        state: crate::worker::pty::PtyState::Running,
        cwd: "/".to_string(),
        checkout: Default::default(),
        launch_root: None,
        launch_commit: None,
        launch_checkout_identity: None,
        session_id: None,
        thread_name: None,
        started_at: 0,
        last_output_at: 0,
        last_error: None,
        busy: false,
        control: crate::worker::pty::SessionControl::Orchestrator,
        origin: crate::worker::pty::SessionOrigin::Orchestrator,
        retained: false,
        name: None,
        attention: None,
        mcp_grant_session: None,
    }
}

#[test]
fn the_sessions_of_one_agent_stay_contiguous() {
    // The agent tier has no row any more, but it still decides the *order*: the
    // rail groups sessions by agent, so an agent's sessions come out as one run
    // rather than interleaved with another's. That is what keeps the tree glyphs
    // (`last`) and the host grouping meaningful without a header to anchor them.
    let app = app();
    let mut seen: Vec<String> = Vec::new();
    let mut current: Option<String> = None;
    for row in app.rail_rows() {
        let RailRow::Session(session) = row else {
            continue;
        };
        let Some(agent_id) = session.agent_id.clone() else {
            continue;
        };
        if current.as_ref() == Some(&agent_id) {
            continue;
        }
        assert!(
            !seen.contains(&agent_id),
            "{agent_id} was already listed and has come back around: {seen:?}"
        );
        seen.push(agent_id.clone());
        current = Some(agent_id);
    }
    assert!(
        !seen.is_empty(),
        "the demo fixture dispatches at least one task"
    );
}

#[test]
fn empty_grouped_sections_do_not_emit_a_header() {
    let rows = app().flatten(
        vec![super::organize::Section {
            header: super::organize::SectionHeader::Group(GroupRailRow {
                label: "/quiet".to_string(),
            }),
            agents: vec![AgentGroup {
                row: AgentRailRow {
                    agent_id: "quiet".to_string(),
                    host_id: String::new(),
                    agent: None,
                    lane_index: None,
                },
                sessions: Vec::new(),
                last_at: 0,
                lane_label: None,
                harness_label: None,
                visible_tasks: 0,
                hidden: 0,
                overflow: false,
            }],
        }],
        Vec::new(),
        &[],
    );

    assert!(
        !rows.iter().any(|row| matches!(row, RailRow::Group(_))),
        "a group without a rendered session must not leave an empty heading"
    );
}

// Unix-only: starts a real child on a real pseudo-terminal via `/bin/sh`,
// which Windows has no equivalent of. The row model under test is
// portable; only this way of standing a session up is not.
#[cfg(unix)]
#[test]
fn a_dispatched_session_and_an_operator_session_are_the_same_row_type() {
    // §A0: the `── your harnesses ──` divider and the second row type are gone.
    // A task row and a PTY row are both `RailRow::Session`, so nothing on the
    // rail separates them into two groups.
    let mut app = hosting_app();
    app.loaded.config.fleet.agent_declarations =
        vec![AgentDeclaration::new("shell", "", "codex", "/")];
    let harnesses = app.local_sessions().expect("hosting").clone();
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

// Unix-only for the same reason as the test below: a real child on a real pty.
#[cfg(unix)]
#[test]
fn a_finished_tasks_session_is_still_there_to_work_in() {
    // The point of retaining a task's session: when the task is done the harness
    // is still running, and the operator can put the cursor on it and carry on
    // in the context it built.
    //
    // The task's own row cannot serve that. It carries no local session, so it
    // resolves no pty — nothing to draw a live screen from and nothing to attach
    // the keyboard to — and the streamed screen stops arriving the moment the
    // task settles, because that lookup goes through the daemon's *running* map.
    // Without a row here, a retained session is alive and reachable by nothing.
    let mut app = hosting_app();
    app.loaded.config.fleet.agent_declarations =
        vec![AgentDeclaration::new("shell", "", "codex", "/")];
    let harnesses = app.local_sessions().expect("hosting").clone();
    let choice = harnesses
        .choices()
        .into_iter()
        .find(|choice| choice.provider == HarnessProvider::Codex)
        .expect("codex is configured");
    // Dispatched, not operator-started: this is the session a task ran in.
    let id = harnesses
        .open_unmanaged(&choice, "/", false)
        .expect("a /bin/sh session starts");
    harnesses
        .sessions
        .set_control(&id, crate::worker::pty::SessionControl::Orchestrator);

    // Its task answers, so the executor retains rather than closes it.
    assert!(harnesses.sessions.retain(&id));

    let row = app
        .rail_rows()
        .into_iter()
        .find_map(|row| match row {
            RailRow::Session(session) if session.session_id() == Some(id.as_str()) => Some(session),
            _ => None,
        })
        .expect("a retained session must have a row to put the cursor on");
    assert_eq!(
        row.session_id(),
        Some(id.as_str()),
        "the row must resolve the pty, or the pane cannot draw it and the \
         keyboard has nothing to attach to"
    );
    harnesses.sessions.shutdown();
}

// Unix-only: starts a real child on a real pseudo-terminal via `/bin/sh`,
// which Windows has no equivalent of. The row model under test is
// portable; only this way of standing a session up is not.
#[cfg(unix)]
#[test]
fn a_session_in_an_undeclared_directory_is_still_listed() {
    // Nothing declares `/`, so the session belongs to no agent — and a harness
    // that is running, costing tokens and invisible is exactly the failure the
    // old separate group existed to prevent.
    let app = hosting_app();
    let harnesses = app.local_sessions().expect("hosting").clone();
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
fn the_host_tree_keeps_a_second_declared_host() {
    let runtime = MockRuntime::empty();
    runtime.set_workers(vec![WorkerInfo {
        id: "studio-claude".into(),
        address: "studio".into(),
        handle: None,
        label: None,
        harness: Some("claude".into()),
        workspace: Some("/work".into()),
        peer_id: None,
        cpu_cores: None,
        memory_total_bytes: None,
        memory_available_bytes: None,
        ip_address: None,
        selected: false,
        roles: Vec::new(),
        budgets: Vec::new(),
        readiness: Vec::new(),
    }]);
    let mut loaded = LoadedConfig::defaults("medulla.tui.json".into());
    loaded.config.link = Some(medulla::config::LinkConfig::default());
    let mut app = App::new(Arc::new(runtime), loaded);
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
        .host_tree()
        .into_iter()
        .map(|host| (host.id, host.kind == medulla::ui::hosts::HostKind::Local))
        .collect();
    assert!(
        hosts.iter().any(|(host_id, _)| host_id == "studio"),
        "the second declared machine stays in the shared tree: {hosts:?}"
    );
    assert!(
        hosts.iter().any(|(_, is_local)| *is_local),
        "the local machine remains in the shared tree: {hosts:?}"
    );
    assert!(
        hosts.len() >= 2,
        "the local and second declared machines both remain: {hosts:?}"
    );
    let local = app.local_host_refs();
    assert_eq!(
        hosts.first().map(|(host_id, _)| host_id.as_str()),
        local.first().map(|host| host.id.as_str()),
        "this device leads, in the order the shared tree lists: {hosts:?}"
    );
}

#[test]
fn the_create_action_is_absent_on_a_device_that_hosts_nothing() {
    // A machine with no host has nowhere to start a session, so the action is
    // missing rather than present and refusing.
    assert!(
        !app().rail_rows().iter().any(RailRow::is_new_session),
        "no host, no create action"
    );
    let rows = hosting_app().rail_rows();
    assert!(
        rows.iter().any(RailRow::is_new_session),
        "hosting, so {NEW_SESSION_LABEL} is offered"
    );
    // And exactly one of it: it used to be emitted per declared agent, which put
    // a button under every group for a flow that no longer asks which agent.
    assert_eq!(
        rows.iter().filter(|row| row.is_new_session()).count(),
        1,
        "one door, not one per agent: {rows:?}"
    );
}

#[test]
fn a_row_answers_for_the_lane_behind_it() {
    let app = app();
    for row in app.rail_rows() {
        match &row {
            RailRow::Session(session) => {
                assert_eq!(row.lane_index(), session.lane_index);
            }
            // The host header and the action row are about no lane and no
            // session.
            RailRow::Host(_) | RailRow::Group(_) | RailRow::NewSession => {
                assert_eq!(row.lane_index(), None);
                assert_eq!(row.session_id(), None);
                assert!(row.task().is_none());
            }
            // A run row is about the session that started it, not a lane of its
            // own.
            RailRow::WorkflowRun(run) => {
                assert_eq!(row.session_id(), Some(run.session_id.as_str()));
                assert_eq!(row.lane_index(), None);
            }
            // The paging control names the lane it pages, and nothing else.
            RailRow::Overflow { lane_index, .. } => {
                assert_eq!(row.lane_index(), Some(*lane_index));
                assert_eq!(row.session_id(), None);
            }
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
            RailRow::Host(_) | RailRow::Group(_) => {
                assert!(!row.selectable(), "a section header is a label")
            }
            RailRow::Session(_)
            | RailRow::NewSession
            | RailRow::WorkflowRun(_)
            | RailRow::Overflow { .. } => {
                assert!(row.selectable())
            }
        }
    }
}
