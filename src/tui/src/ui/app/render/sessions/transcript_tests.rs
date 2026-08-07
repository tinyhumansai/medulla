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

use crate::ui::app::rail::{HostRailRow, RailRow};
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
        session: None,
        workflow_run: None,
    };
    let mut terminal = Terminal::new(TestBackend::new(100, 12)).unwrap();
    terminal
        .draw(|frame| app.draw_sessions_pane(frame, Rect::new(0, 0, 100, 12), &selection))
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
        .draw(|frame| app.draw_sessions_pane(frame, Rect::new(0, 0, 100, 12), &selection))
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
    pane_for_size(row, 90, 20)
}

/// Draw one rail row's pane at a particular terminal size.
fn pane_for_size(row: RailRow, width: u16, height: u16) -> String {
    let runtime: Arc<dyn Runtime> = Arc::new(MockRuntime::empty());
    let mut app = App::new(runtime, LoadedConfig::defaults("medulla.tui.json".into()));
    // Derived from the row rather than defaulted, exactly as `sessions_selection`
    // does it: the pane chooses what to draw from this field, so a fixture that
    // left it empty would test a selection the app never builds.
    let workflow_run = row.workflow_run().cloned();
    let selection = Selection {
        rows: vec![row],
        active: 0,
        lanes: vec![orchestrator_lane()],
        // What the fix makes representable: a row with no lane of its own. It
        // used to be `0`, which is this lane.
        lane_index: None,
        task: None,
        session: None,
        workflow_run,
    };
    let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
    terminal
        .draw(|frame| app.draw_sessions_pane(frame, Rect::new(0, 0, width, height), &selection))
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

#[test]
fn a_row_with_no_lane_never_renders_the_orchestrators_stream() {
    // The bug: `selection.lane()` fell back to lane 0 — the orchestrator's — so
    // selecting the action row or a host header showed the orchestrator
    // thinking, attributed to a row that had not thought anything.
    for (what, row) in [
        ("the create action", RailRow::NewSession),
        (
            "a host header",
            RailRow::Host(HostRailRow {
                host_id: "studio".into(),
                label: "studio".into(),
                local: false,
            }),
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
fn a_host_row_and_the_action_row_say_what_they_are() {
    let host = pane_for(RailRow::Host(HostRailRow {
        host_id: "studio".into(),
        label: "studio".into(),
        local: false,
    }));
    assert!(host.contains("host · studio"), "{host}");
    assert!(host.contains("remote"), "local or remote: {host}");

    let new_session = pane_for(RailRow::NewSession);
    assert!(new_session.contains("New session"), "{new_session}");
    assert!(
        new_session.contains("harness type"),
        "and what it will ask: {new_session}"
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
        app.rail_index = index;
        let selection = app.sessions_selection();
        assert!(
            selection.lane_index.is_none() && selection.lane().is_none(),
            "row {index} borrowed a lane it does not have"
        );
    }
}

/// A run reported by a session, as the control plane hands one to the rail.
fn reported_run() -> RailRow {
    RailRow::WorkflowRun(crate::ui::app::rail::WorkflowRunRailRow {
        session_id: "pty-abcdef0123456789".into(),
        run: medulla::control_socket::HarnessRun {
            run_id: "run-77".into(),
            workflow_id: "release-train".into(),
            status: medulla::control_socket::HarnessRunStatus::Running,
            started_at: 1_000,
            updated_at: 4_000,
            detail: Some("running Terminal · $ cargo test".into()),
            frames: vec![medulla::control_socket::HarnessRunFrame {
                node: Some("verify".into()),
                text: "running the test suite".into(),
            }],
        },
        last: true,
    })
}

#[test]
fn a_run_row_draws_the_run_rather_than_the_session_that_started_it() {
    // The whole of the second complaint: arrowing onto a run showed the parent
    // harness's terminal, because the row answered `session_id()` with its
    // parent's. The cursor is on the run, so the pane is the run's.
    let pane = pane_for(reported_run());

    assert!(pane.contains("release-train"), "{pane}");
    assert!(pane.contains("running"), "{pane}");
    // The frames the run has reported, in the same vocabulary the Workflows
    // tab's step preview uses — which classifies this one as a tool call and
    // renders it under its own glyph rather than verbatim.
    assert!(pane.contains("test suite"), "{pane}");
    // The session is named as provenance, not drawn as a terminal.
    assert!(pane.contains("abcdef01"), "{pane}");
    assert!(!pane.contains(ORCHESTRATOR_ONLY), "{pane}");
}

#[test]
fn a_narrow_uninstalled_run_keeps_its_newest_progress_visible() {
    let mut row = reported_run();
    let RailRow::WorkflowRun(run) = &mut row else {
        unreachable!("the fixture is a workflow run");
    };
    run.run.frames = (0..12)
        .map(|index| medulla::control_socket::HarnessRunFrame {
            node: None,
            text: format!("old progress frame {index}"),
        })
        .collect();
    run.run
        .frames
        .push(medulla::control_socket::HarnessRunFrame {
            node: None,
            text: "newest progress".into(),
        });

    // The description wraps to many physical rows at this width. A logical-line
    // budget would choose an old frame tail that never reaches the viewport.
    let pane = pane_for_size(row, 28, 20);
    assert!(pane.contains("newest progress"), "{pane}");
}

#[test]
fn a_short_uninstalled_run_prioritizes_progress_over_its_live_header() {
    let mut row = reported_run();
    let RailRow::WorkflowRun(run) = &mut row else {
        unreachable!("the fixture is a workflow run");
    };
    run.run.frames = vec![medulla::control_socket::HarnessRunFrame {
        node: None,
        text: "newest progress".into(),
    }];

    // The fixed context leaves one line. The live header used to consume it and
    // force this frame below the viewport, exactly when the run was too short
    // to make the status alone useful.
    let pane = pane_for_size(row, 120, 9);
    assert!(pane.contains("newest progress"), "{pane}");
}

#[test]
fn a_run_row_names_no_session_so_nothing_attaches_to_its_parent() {
    // `session_id()` means "the session this row is". A run row answering with
    // its parent made the pane draw that harness, a click attach to it, and
    // `select_session_row` able to land on a run.
    let row = reported_run();
    assert!(row.session_id().is_none());
    assert!(row.workflow_run().is_some(), "the run is still reachable");
}

#[test]
fn selecting_a_run_points_the_workflow_state_at_it() {
    // The mirror is what lets the Sessions pane reuse the Workflows tab's canvas:
    // that canvas reads the overlay out of the workflow state, so selecting a
    // run here has to move it.
    let runtime: Arc<dyn Runtime> = Arc::new(MockRuntime::empty());
    let mut app = App::new(runtime, LoadedConfig::defaults("medulla.tui.json".into()));
    app.workflows = vec![medulla::workflows::WorkflowSummary {
        id: "release-train".into(),
        name: "Release train".into(),
        description: String::new(),
        enabled: true,
        node_count: 0,
        trigger_kind: None,
        inputs: Vec::new(),
    }];

    let mut selection = Selection {
        rows: vec![reported_run()],
        active: 0,
        lanes: vec![orchestrator_lane()],
        lane_index: None,
        task: None,
        session: None,
        workflow_run: Some(crate::ui::app::rail::WorkflowRunRailRow {
            session_id: "pty-abcdef0123456789".into(),
            run: medulla::control_socket::HarnessRun {
                run_id: "run-77".into(),
                workflow_id: "release-train".into(),
                status: medulla::control_socket::HarnessRunStatus::Running,
                started_at: 1_000,
                updated_at: 4_000,
                detail: None,
                frames: Vec::new(),
            },
            last: true,
        }),
    };

    app.mirror_selected_workflow_run(selection.workflow_run.as_ref());
    assert_eq!(app.wf.mirrored_run.as_deref(), Some("run-77"));
    assert_eq!(app.wf.mirrored_run_updated_at, Some(4_000));

    // A run keeps its id as it moves through the graph. Without a selected
    // graph node there is no evidence that this newer report still describes
    // the preview the operator was reading, so reset to its live tail.
    app.wf.preview_scroll = 7;
    selection.workflow_run.as_mut().unwrap().run.updated_at = 5_000;
    app.mirror_selected_workflow_run(selection.workflow_run.as_ref());
    assert_eq!(app.wf.mirrored_run_updated_at, Some(5_000));
    assert_eq!(app.wf.preview_scroll, 0);

    // Stepping off a run clears the mark, so returning to it re-syncs rather
    // than trusting a graph the store may have changed underneath.
    let empty = Selection {
        rows: Vec::new(),
        active: 0,
        lanes: vec![orchestrator_lane()],
        lane_index: None,
        task: None,
        session: None,
        workflow_run: None,
    };
    app.mirror_selected_workflow_run(empty.workflow_run.as_ref());
    assert!(app.wf.mirrored_run.is_none());
    assert!(app.wf.mirrored_run_updated_at.is_none());
}

#[test]
fn the_pane_diff_takes_the_harness_screens_real_estate() {
    // The swap is a tab switch inside one pane: the same rectangle that was
    // painting the terminal now paints the diff, titled with the way back. A
    // diff opened *beside* the harness would be two half-width panes, and a
    // unified patch is unreadable in one of those.
    use crate::ui::app::changes::types::ChangedFile;
    use crate::ui::app::types::PaneView;

    let runtime: Arc<dyn Runtime> = Arc::new(MockRuntime::empty());
    let mut app = App::new(runtime, LoadedConfig::defaults("medulla.tui.json".into()));
    app.changes.root = Some(std::path::PathBuf::from("/repo"));
    app.changes.baseline = Some("baseline".to_owned());
    app.changes.files = vec![ChangedFile {
        status: "M".into(),
        path: std::path::PathBuf::from("src/lib.rs"),
        origins: Vec::new(),
    }];
    app.changes.patch = vec!["@@ -1,1 +1,1 @@".to_owned(), "+added line".to_owned()];
    app.pane_view = PaneView::Diff;

    let selection = Selection {
        rows: Vec::new(),
        active: 0,
        lanes: Vec::new(),
        lane_index: None,
        task: None,
        session: Some("live-session".to_owned()),
        workflow_run: None,
    };
    let mut terminal = Terminal::new(TestBackend::new(120, 20)).unwrap();
    terminal
        .draw(|frame| app.draw_sessions_pane(frame, Rect::new(0, 0, 120, 20), &selection))
        .unwrap();
    let output: String = terminal
        .backend()
        .buffer()
        .content()
        .iter()
        .map(|cell| cell.symbol())
        .collect();

    assert!(output.contains("From app launch"), "{output}");
    assert!(output.contains("d harness"), "the way back: {output}");
    assert!(output.contains("src/lib.rs"), "{output}");
    assert!(output.contains("+added line"), "{output}");
}

#[test]
fn moving_the_cursor_off_a_session_puts_its_pane_back_to_the_harness() {
    // A view is opened about one session. Left standing across a selection
    // change it would describe the previous harness's work under the new one's
    // title — the two are different repositories as often as not.
    use crate::ui::app::rail::tests::{hosting_app, stub_session};
    use crate::ui::app::rail::SessionRailRow;
    use crate::ui::app::types::PaneView;

    let row_for = |id: &str| {
        RailRow::Session(Box::new(SessionRailRow {
            agent_id: None,
            lane_index: None,
            task: None,
            local: Some(stub_session(id)),
            last: true,
        }))
    };
    let selection_on = |id: &str| Selection {
        rows: vec![row_for(id)],
        active: 0,
        lanes: Vec::new(),
        lane_index: None,
        task: None,
        session: None,
        workflow_run: None,
    };

    let mut app = hosting_app();
    app.pane_view = PaneView::Diff;
    app.pane_view_session = Some("first-session".to_owned());

    // The same session: the operator has not gone anywhere, so the view stays.
    let mut selection = selection_on("first-session");
    app.resolve_selected_session(&mut selection);
    assert_eq!(app.pane_view, PaneView::Diff);

    let mut selection = selection_on("second-session");
    app.resolve_selected_session(&mut selection);
    assert_eq!(app.pane_view, PaneView::Harness);
    assert_eq!(app.pane_view_session.as_deref(), Some("second-session"));
}
