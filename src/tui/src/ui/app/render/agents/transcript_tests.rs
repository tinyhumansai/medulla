//! Rendering regressions for session context in the transcript header, and for
//! the rows that have no transcript at all.

use std::sync::Arc;

use medulla::config::LoadedConfig;
use medulla::harness_work::{HarnessSessionInfo, WorkSnapshot};
use medulla::runtime::mock::MockRuntime;
use medulla::runtime::{AgentDeclaration, Runtime};
use medulla::ui::agents::{AgentLane, AgentRole, TurnBlock};
use ratatui::backend::TestBackend;
use ratatui::layout::Rect;
use ratatui::Terminal;

use crate::ui::app::rail::{AgentRailRow, HostRailRow, RailRow};
use crate::ui::app::App;

use super::types::Selection;

#[test]
fn descriptorless_lanes_still_show_their_pull_request_context() {
    let runtime: Arc<dyn Runtime> = Arc::new(MockRuntime::empty());
    let mut app = App::new(runtime, LoadedConfig::defaults("medulla.tui.json".into()));
    let lane = AgentLane {
        key: "worker".into(),
        label: "worker".into(),
        role: AgentRole::Agent,
        turns: Vec::new(),
        last_at: 0,
        tasks: Vec::new(),
        context_tokens: None,
        usage: Default::default(),
        harness_label: None,
        agent_id: Some("worker".into()),
        session_id: None,
        parent_agent_id: None,
        descriptor: None,
        active_tasks: 0,
        work: Some(Box::new(WorkSnapshot {
            info: HarnessSessionInfo {
                cwd: Some("/repo/worktrees/fix-context".into()),
                branch: Some("fix-context".into()),
                pull_request: Some("https://github.com/acme/repo/pull/42".into()),
                ..Default::default()
            },
            ..Default::default()
        })),
    };
    let mut selection = Selection {
        rows: Vec::new(),
        active: 0,
        lanes: vec![lane],
        lane_index: Some(0),
        task: None,
        on_orchestrator: false,
        harness: None,
    };
    let mut terminal = Terminal::new(TestBackend::new(100, 12)).unwrap();
    terminal
        .draw(|frame| app.draw_agents_pane(frame, Rect::new(0, 0, 100, 12), &selection))
        .unwrap();
    let output: String = terminal
        .backend()
        .buffer()
        .content()
        .iter()
        .map(|cell| cell.symbol())
        .collect();

    assert!(output.contains("branch fix-context"), "{output}");
    assert!(
        output.contains("dir /repo/worktrees/fix-context"),
        "{output}"
    );
    assert!(output.contains("PR 42"), "{output}");

    selection.lanes[0].work.as_mut().unwrap().info.pull_request = None;
    terminal
        .draw(|frame| app.draw_agents_pane(frame, Rect::new(0, 0, 100, 12), &selection))
        .unwrap();
    let output_without_pr: String = terminal
        .backend()
        .buffer()
        .content()
        .iter()
        .map(|cell| cell.symbol())
        .collect();
    assert!(output_without_pr.contains("branch fix-context"));
    assert!(!output_without_pr.contains("PR 42"));
}

/// A marker only the orchestrator's lane ever renders.
const ORCHESTRATOR_ONLY: &str = "ORCHESTRATORTHINKING";

/// Lane 0 — the orchestrator's — with a body no other row may show.
fn orchestrator_lane() -> AgentLane {
    AgentLane {
        key: "orchestrator".into(),
        label: "orchestrator".into(),
        role: AgentRole::Orchestrator,
        turns: vec![TurnBlock {
            at: 1,
            header: ORCHESTRATOR_ONLY.into(),
            header_color: None,
            reasoning: Some(ORCHESTRATOR_ONLY.into()),
            content: Some(ORCHESTRATOR_ONLY.into()),
            tools: Vec::new(),
        }],
        last_at: 1,
        tasks: Vec::new(),
        context_tokens: None,
        usage: Default::default(),
        harness_label: None,
        agent_id: None,
        session_id: None,
        parent_agent_id: None,
        descriptor: None,
        active_tasks: 0,
        work: None,
    }
}

/// Draw one rail row's pane and return everything it painted.
fn pane_for(row: RailRow) -> String {
    let runtime: Arc<dyn Runtime> = Arc::new(MockRuntime::empty());
    let mut app = App::new(runtime, LoadedConfig::defaults("medulla.tui.json".into()));
    let selection = Selection {
        rows: vec![row],
        active: 0,
        lanes: vec![orchestrator_lane()],
        // What the fix makes representable: a row with no lane of its own. It
        // used to be `0`, which is this lane.
        lane_index: None,
        task: None,
        on_orchestrator: false,
        harness: None,
    };
    let mut terminal = Terminal::new(TestBackend::new(90, 20)).unwrap();
    terminal
        .draw(|frame| app.draw_agents_pane(frame, Rect::new(0, 0, 90, 20), &selection))
        .unwrap();
    terminal
        .backend()
        .buffer()
        .content()
        .iter()
        .map(|cell| cell.symbol())
        .collect()
}

/// A declared agent with nothing dispatched to it: a row, and no lane.
fn idle_agent() -> AgentRailRow {
    AgentRailRow {
        agent_id: "api-claude".into(),
        host_id: String::new(),
        agent: Some(medulla::ui::hosts::HostAgentRow {
            agent_id: "api-claude".into(),
            label: "API".into(),
            harness: Some("claude".into()),
            workspace: Some("/work/api".into()),
            roles: vec!["reviewer".into()],
            max_sessions: Some(1),
            declared: true,
            editable: true,
            live: false,
            selected: false,
        }),
        lane_index: None,
    }
}

#[test]
fn a_row_with_no_lane_never_renders_the_orchestrators_stream() {
    // The bug: `selection.lane()` fell back to lane 0 — the orchestrator's — so
    // selecting `+ New agent`, a host header, or an agent nothing had been
    // dispatched to showed the orchestrator thinking, attributed to a row that
    // had not thought anything.
    for (what, row) in [
        ("the create action", RailRow::NewAgent),
        (
            "a host header",
            RailRow::Host(HostRailRow {
                host_id: "studio".into(),
                label: "studio".into(),
                local: false,
            }),
        ),
        ("an idle agent", RailRow::Agent(idle_agent())),
        (
            "the per-agent action",
            RailRow::NewSession {
                agent_id: "api-claude".into(),
            },
        ),
    ] {
        let output = pane_for(row);
        assert!(
            !output.contains(ORCHESTRATOR_ONLY),
            "{what} showed lane 0's stream: {output}"
        );
    }
}

#[test]
fn an_idle_agent_describes_itself_instead() {
    let output = pane_for(RailRow::Agent(idle_agent()));
    assert!(output.contains("agent · API"), "{output}");
    assert!(output.contains("api-claude"), "the id: {output}");
    assert!(output.contains("claude"), "the harness: {output}");
    assert!(output.contains("/work/api"), "the workspace: {output}");
    assert!(output.contains("reviewer"), "its roles: {output}");
    assert!(output.contains("No sessions yet"), "the count: {output}");
    assert!(
        output.contains("new session"),
        "and how to start one: {output}"
    );
}

#[test]
fn a_host_row_and_the_action_rows_say_what_they_are() {
    let host = pane_for(RailRow::Host(HostRailRow {
        host_id: "studio".into(),
        label: "studio".into(),
        local: false,
    }));
    assert!(host.contains("host · studio"), "{host}");
    assert!(host.contains("remote"), "local or remote: {host}");

    let new_agent = pane_for(RailRow::NewAgent);
    assert!(new_agent.contains("New agent"), "{new_agent}");
    assert!(new_agent.contains("Declare an agent"), "{new_agent}");

    let new_session = pane_for(RailRow::NewSession {
        agent_id: "api-claude".into(),
    });
    assert!(new_session.contains("new session"), "{new_session}");
    assert!(
        new_session.contains("api-claude"),
        "named for its agent: {new_session}"
    );
}

#[test]
fn the_selection_gives_a_laneless_row_no_lane_at_all() {
    // The guard itself, one level below the render: the rail's own rows resolve
    // to `None` rather than to lane 0, so no caller can inherit a stream.
    let mut app = crate::ui::app::rail::tests::hosting_app();
    app.loaded.config.fleet.agent_declarations = vec![AgentDeclaration::new(
        "idle-agent",
        "",
        "claude",
        "/work/idle",
    )];
    let rows = app.rail_rows();
    for (index, row) in rows.iter().enumerate() {
        if row.lane_index().is_some() {
            continue;
        }
        app.agent_index = index;
        let selection = app.agents_selection();
        assert!(
            selection.lane_index.is_none() && selection.lane().is_none(),
            "row {index} borrowed a lane it does not have"
        );
        if !matches!(row, RailRow::Lane(_)) {
            assert!(
                !selection.on_orchestrator,
                "row {index} is not the conversation"
            );
        }
    }
}
