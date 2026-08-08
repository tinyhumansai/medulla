//! How the sidebar's grouping and sorting preferences change what the rail
//! lists — and, as much to the point, what they must not change: every agent
//! keeps a row and every session stays under its own agent whichever way the
//! two settings are turned.

use medulla::config::{SidebarGrouping, SidebarSort};
use medulla::runtime::AgentDeclaration;
use medulla::ui::hosts::HostAgentRow;

use super::super::tests::{app, stub_session};
use super::super::{
    AgentGroup, AgentRailRow, GroupRailRow, HostGroup, HostRailRow, SessionRailRow,
};
use super::{flatten_agents, order_sections, sort_agents, sort_sessions, Section, SectionHeader};
use crate::ui::app::App;

/// Three agents across two checkouts and two harnesses, in declaration order.
fn app_with_agents() -> App {
    let mut app = app();
    app.loaded.config.fleet.agent_declarations = vec![
        AgentDeclaration::new("zed", "", "claude", "/work/beta"),
        AgentDeclaration::new("acorn", "", "codex", "/work/alpha"),
        AgentDeclaration::new("mint", "", "claude", "/work/alpha"),
    ];
    app
}

/// The configured sections for the declared agents, without unrelated mock lanes.
fn sections(app: &App) -> Vec<(Option<String>, Vec<String>)> {
    let agents = app
        .loaded
        .config
        .fleet
        .agent_declarations
        .iter()
        .map(|declaration| AgentGroup {
            row: AgentRailRow {
                agent_id: declaration.agent_id.clone(),
                host_id: String::new(),
                // `organize` receives the already placed tree in production,
                // whose rows carry their declaration-derived workspace and
                // harness. Keep this fixture faithful so path and harness
                // grouping exercise the values the implementation reads.
                agent: Some(HostAgentRow {
                    agent_id: declaration.agent_id.clone(),
                    label: declaration.agent_id.clone(),
                    harness: Some(declaration.harness.clone()),
                    workspace: Some(declaration.workspace.path.clone()),
                    roles: Vec::new(),
                    max_sessions: None,
                    declared: true,
                    editable: true,
                    live: false,
                    selected: false,
                }),
                lane_index: None,
            },
            sessions: Vec::new(),
            last_at: 0,
            lane_label: None,
            harness_label: None,
            visible_tasks: 0,
            hidden: 0,
            overflow: false,
        })
        .collect();
    super::organize(
        vec![HostGroup {
            row: HostRailRow {
                host_id: String::new(),
                label: "local".into(),
                local: true,
            },
            agents,
        }],
        &app.loaded.config.fleet.agent_declarations,
        app.loaded.config.appearance.sidebar_grouping,
        app.loaded.config.appearance.sidebar_sort,
    )
    .into_iter()
    .map(|section| {
        let header = match section.header {
            SectionHeader::Host(host) => Some(host.label),
            SectionHeader::Group(group) => Some(group.label),
            SectionHeader::None => None,
        };
        let agents = section
            .agents
            .into_iter()
            .map(|agent| agent.row.agent_id)
            .collect();
        (header, agents)
    })
    .collect()
}

#[test]
fn the_default_leaves_one_machines_tree_unsectioned() {
    // The setting exists to be changed, not to change what an operator who has
    // never opened it sees: with one host and the default grouping the rail is
    // the flat list it was before grouping was configurable, in declaration
    // order.
    let app = app_with_agents();
    assert_eq!(
        app.loaded.config.appearance.sidebar_grouping,
        SidebarGrouping::Host
    );
    assert_eq!(
        sections(&app),
        vec![(None, vec!["zed".into(), "acorn".into(), "mint".into()])]
    );
}

#[test]
fn grouping_by_path_heads_each_checkout_once() {
    let mut app = app_with_agents();
    app.loaded.config.appearance.sidebar_grouping = SidebarGrouping::Path;

    assert_eq!(
        sections(&app),
        vec![
            (
                Some("/work/alpha".into()),
                vec!["acorn".into(), "mint".into()]
            ),
            (Some("/work/beta".into()), vec!["zed".into()]),
        ],
        "one header per directory, alphabetical, with its agents under it"
    );
}

#[test]
fn grouping_by_path_ignores_a_trailing_separator() {
    let mut app = app_with_agents();
    app.loaded.config.appearance.sidebar_grouping = SidebarGrouping::Path;
    app.loaded.config.fleet.agent_declarations[2].workspace.path = "/work/alpha/".into();

    assert_eq!(
        sections(&app),
        vec![
            (
                Some("/work/alpha".into()),
                vec!["acorn".into(), "mint".into()]
            ),
            (Some("/work/beta".into()), vec!["zed".into()]),
        ]
    );
}

#[test]
fn grouping_by_harness_sections_by_the_cli_each_agent_runs() {
    let mut app = app_with_agents();
    app.loaded.config.appearance.sidebar_grouping = SidebarGrouping::Harness;

    let sections = sections(&app);
    let claude = sections
        .iter()
        .find(|(label, _)| label.as_deref() == Some("claude"))
        .expect("the claude agents are sectioned together");
    assert_eq!(claude.1, vec!["zed".to_string(), "mint".to_string()]);
    assert!(sections
        .iter()
        .any(|(label, agents)| label.as_deref() == Some("codex") && agents == &["acorn"]));
}

#[test]
fn grouping_by_harness_ignores_ascii_case() {
    let mut app = app_with_agents();
    app.loaded.config.appearance.sidebar_grouping = SidebarGrouping::Harness;
    app.loaded.config.fleet.agent_declarations[2].harness = "Claude".into();

    assert_eq!(
        sections(&app),
        vec![
            (Some("claude".into()), vec!["zed".into(), "mint".into()]),
            (Some("codex".into()), vec!["acorn".into()]),
        ],
        "the first configured spelling remains the label for one shared harness section"
    );
}

#[test]
fn grouping_by_none_lists_every_agent_without_a_header() {
    let mut app = app_with_agents();
    app.loaded.config.appearance.sidebar_grouping = SidebarGrouping::None;

    assert_eq!(
        sections(&app),
        vec![(None, vec!["zed".into(), "acorn".into(), "mint".into()])]
    );
}

#[test]
fn no_grouping_loses_an_agent() {
    // The one property that has to hold across all four: sectioning is a
    // presentation of the same tree, so the set of agents on the rail is the
    // same set however it is grouped.
    let mut app = app_with_agents();
    let mut seen: Vec<Vec<String>> = Vec::new();
    for grouping in SidebarGrouping::ALL {
        app.loaded.config.appearance.sidebar_grouping = grouping;
        let mut agents: Vec<String> = sections(&app)
            .into_iter()
            .flat_map(|(_, agents)| agents)
            .collect();
        agents.sort();
        seen.push(agents);
    }
    assert!(
        seen.windows(2).all(|pair| pair[0] == pair[1]),
        "every grouping lists the same agents: {seen:?}"
    );
}

#[test]
fn sorting_by_name_is_alphabetical_within_a_section() {
    let mut app = app_with_agents();
    app.loaded.config.appearance.sidebar_sort = SidebarSort::Name;

    assert_eq!(
        sections(&app),
        vec![(None, vec!["acorn".into(), "mint".into(), "zed".into()])]
    );
}

/// A session row for a live pty with the given start and last-output times.
fn session(id: &str, started_at: i64, last_output_at: i64) -> SessionRailRow {
    let mut local = stub_session(id);
    local.started_at = started_at;
    local.last_output_at = last_output_at;
    local.name = Some(id.to_string());
    SessionRailRow {
        agent_id: Some("agent".into()),
        lane_index: None,
        task: None,
        local: Some(local),
        last: false,
    }
}

/// The ids of `sessions` after sorting, for the session-order assertions.
fn sorted(mut sessions: Vec<SessionRailRow>, sort: SidebarSort) -> Vec<String> {
    sort_sessions(&mut sessions, sort);
    sessions
        .into_iter()
        .filter_map(|session| session.session_id().map(str::to_string))
        .collect()
}

#[test]
fn created_sorts_sessions_oldest_first() {
    let rows = vec![
        session("middle", 200, 500),
        session("oldest", 100, 900),
        session("newest", 300, 100),
    ];
    assert_eq!(
        sorted(rows, SidebarSort::Created),
        vec!["oldest", "middle", "newest"],
        "the default is the order a session list grows in"
    );
}

#[test]
fn created_keeps_task_rows_in_fold_order_after_pty_enrichment() {
    let mut served = active_agent("served", 0)
        .sessions
        .pop()
        .expect("one task session");
    let mut local = stub_session("pty");
    local.started_at = 100;
    served.local = Some(local);
    let task_only = active_agent("task-only", 0)
        .sessions
        .pop()
        .expect("one task session");

    let mut sessions = vec![served, task_only];
    sort_sessions(&mut sessions, SidebarSort::Created);

    assert_eq!(
        sessions
            .iter()
            .filter_map(|session| session.task.as_ref().map(|task| task.task_id.as_str()))
            .collect::<Vec<_>>(),
        vec!["task-served", "task-task-only"],
        "a live PTY must not move a task-backed row out of the fold's order"
    );
}

#[test]
fn recent_sorts_by_the_last_thing_a_session_did() {
    let rows = vec![
        session("quiet", 300, 100),
        session("loud", 100, 900),
        session("middling", 200, 500),
    ];
    assert_eq!(
        sorted(rows, SidebarSort::Recent),
        vec!["loud", "middling", "quiet"],
        "most recent output first, whatever order they started in"
    );
}

#[test]
fn name_sorts_sessions_by_what_the_operator_called_them() {
    let rows = vec![
        session("zulu", 100, 100),
        session("alpha", 200, 200),
        session("mike", 300, 300),
    ];
    assert_eq!(
        sorted(rows, SidebarSort::Name),
        vec!["alpha", "mike", "zulu"]
    );
}

#[test]
fn name_sorts_local_sessions_by_their_visible_thread_titles() {
    let mut zulu = session("zulu", 100, 100);
    let mut alpha = session("alpha", 200, 200);
    for session in [&mut zulu, &mut alpha] {
        let local = session.local.as_mut().expect("live pty session");
        local.name = None;
    }
    zulu.local.as_mut().expect("live pty session").thread_name = Some("Zulu thread".into());
    alpha.local.as_mut().expect("live pty session").thread_name = Some("Alpha thread".into());

    assert_eq!(
        sorted(vec![zulu, alpha], SidebarSort::Name),
        vec!["alpha", "zulu"],
        "local sessions without launch names sort by the titles visible in the rail"
    );
}

/// An agent group with one task whose activity timestamp controls recent order.
fn active_agent(label: &str, last_at: i64) -> AgentGroup {
    AgentGroup {
        row: AgentRailRow {
            agent_id: label.into(),
            host_id: String::new(),
            agent: None,
            lane_index: None,
        },
        sessions: vec![SessionRailRow {
            agent_id: Some(label.into()),
            lane_index: None,
            task: Some(medulla::ui::agents::TaskState {
                task_id: format!("task-{label}"),
                status: medulla::ui::agents::TaskStatus::Running,
                turns: 0,
                last_at,
                turn_blocks: Vec::new(),
                attention: None,
                question_id: None,
                work: None,
            }),
            local: None,
            last: false,
        }],
        last_at,
        lane_label: None,
        harness_label: None,
        visible_tasks: 0,
        hidden: 0,
        overflow: false,
    }
}

/// An agent group whose activity is reported only at the lane level.
fn peer_agent(label: &str, last_at: i64) -> AgentGroup {
    AgentGroup {
        row: AgentRailRow {
            agent_id: label.into(),
            host_id: String::new(),
            agent: None,
            lane_index: None,
        },
        sessions: Vec::new(),
        last_at,
        lane_label: Some(label.into()),
        harness_label: None,
        visible_tasks: 0,
        hidden: 0,
        overflow: false,
    }
}

#[test]
fn recent_sorts_agents_by_their_most_recent_session() {
    let mut agents = vec![active_agent("quiet", 100), active_agent("loud", 900)];

    sort_agents(&mut agents, SidebarSort::Recent);

    assert_eq!(
        agents
            .iter()
            .map(|agent| agent.row.agent_id.as_str())
            .collect::<Vec<_>>(),
        vec!["loud", "quiet"]
    );
}

#[test]
fn recent_sorts_agents_by_peer_lane_activity() {
    let mut agents = vec![active_agent("task", 500), peer_agent("peer", 900)];

    sort_agents(&mut agents, SidebarSort::Recent);

    assert_eq!(
        agents
            .iter()
            .map(|agent| agent.row.agent_id.as_str())
            .collect::<Vec<_>>(),
        vec!["peer", "task"]
    );
}

#[test]
fn name_sorts_lane_only_agents_by_their_displayed_labels() {
    let mut zulu = peer_agent("opaque-a", 0);
    zulu.lane_label = Some("Zulu".into());
    let mut alpha = peer_agent("opaque-z", 0);
    alpha.lane_label = Some("Alpha".into());

    let mut agents = vec![zulu, alpha];
    sort_agents(&mut agents, SidebarSort::Name);

    assert_eq!(agents[0].row.agent_id, "opaque-z");
}

#[test]
fn harness_grouping_uses_a_lane_only_agents_reported_harness() {
    let mut peer = peer_agent("peer", 0);
    peer.harness_label = Some("CODEX".into());

    let sections = super::by_key(vec![peer], super::agent_harness, str::eq_ignore_ascii_case);

    assert!(matches!(
        &sections[0].header,
        SectionHeader::Group(group) if group.label == "CODEX"
    ));
}

#[test]
fn recent_sorts_sections_by_peer_lane_activity() {
    let mut sections = vec![
        Section {
            header: SectionHeader::Group(GroupRailRow {
                label: "/work/task".into(),
            }),
            agents: vec![active_agent("task", 500)],
        },
        Section {
            header: SectionHeader::Group(GroupRailRow {
                label: "/work/peer".into(),
            }),
            agents: vec![peer_agent("peer", 900)],
        },
    ];

    order_sections(&mut sections, SidebarSort::Recent);

    assert!(matches!(
        &sections[0].header,
        SectionHeader::Group(group) if group.label == "/work/peer"
    ));
}

#[test]
fn recent_sorts_path_sections_by_their_most_recent_agent() {
    let mut sections = vec![
        Section {
            header: SectionHeader::Group(GroupRailRow {
                label: "/work/older".into(),
            }),
            agents: vec![active_agent("older", 100)],
        },
        Section {
            header: SectionHeader::Group(GroupRailRow {
                label: "/work/newer".into(),
            }),
            agents: vec![active_agent("newer", 900)],
        },
    ];

    order_sections(&mut sections, SidebarSort::Recent);

    assert!(matches!(
        &sections[0].header,
        SectionHeader::Group(group) if group.label == "/work/newer"
    ));
}

#[test]
fn created_keeps_declaration_order_after_flattening_hosts() {
    let declarations = vec![
        AgentDeclaration::new("first", "host-one", "codex", "/one"),
        AgentDeclaration::new("second", "host-two", "codex", "/two"),
        AgentDeclaration::new("third", "host-one", "codex", "/one"),
    ];
    let hosts = vec![
        HostGroup {
            row: HostRailRow {
                host_id: "host-one".into(),
                label: "one".into(),
                local: true,
            },
            agents: vec![active_agent("first", 0), active_agent("third", 0)],
        },
        HostGroup {
            row: HostRailRow {
                host_id: "host-two".into(),
                label: "two".into(),
                local: false,
            },
            agents: vec![active_agent("second", 0)],
        },
    ];

    let agents = flatten_agents(hosts, &declarations, SidebarSort::Created);

    assert_eq!(
        agents
            .iter()
            .map(|agent| agent.row.agent_id.as_str())
            .collect::<Vec<_>>(),
        vec!["first", "second", "third"],
        "grouping must not turn A(host one), B(host two), C(host one) into A, C, B"
    );
}
