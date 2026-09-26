//! Tests for session paging, retention, and overflow rendering.

use super::super::organize::sort_sessions;
use super::super::tests::app;
use super::super::{AgentGroup, AgentRailRow, RailAnchor, RailRow, SessionRailRow};
use super::push_group;
use medulla::config::SidebarSort;
use medulla::control_socket::{HarnessRunStatus, RunReport};
use medulla::ui::agents::{AgentLane, AgentRole, TaskState, TaskStatus};

fn task_row(task_id: &str) -> SessionRailRow {
    SessionRailRow {
        agent_id: Some("shell".to_string()),
        lane_index: Some(0),
        task: Some(TaskState {
            task_id: task_id.to_string(),
            status: TaskStatus::Running,
            turns: 0,
            last_at: 0,
            turn_blocks: Vec::new(),
            attention: None,
            question_id: None,
            work: None,
        }),
        local: None,
        last: false,
    }
}

fn group(sessions: Vec<SessionRailRow>) -> AgentGroup {
    AgentGroup {
        row: AgentRailRow {
            agent_id: "shell".to_string(),
            host_id: String::new(),
            agent: None,
            lane_index: Some(0),
        },
        sessions,
        last_at: 0,
        lane_label: None,
        harness_label: None,
        visible_tasks: 0,
        hidden: 0,
        overflow: false,
    }
}

fn shell_lane() -> AgentLane {
    AgentLane {
        key: "agent:shell".into(),
        label: "shell".into(),
        role: AgentRole::Agent,
        turns: Vec::new(),
        last_at: 0,
        tasks: Vec::new(),
        context_tokens: None,
        usage: Default::default(),
        harness_label: None,
        agent_id: Some("shell".into()),
        session_id: None,
        parent_agent_id: None,
        descriptor: None,
        active_tasks: 0,
        work: None,
    }
}

#[test]
fn split_fold_derives_the_first_page_counts_from_the_lane_tasks() {
    let mut lane = shell_lane();
    lane.tasks = (0..11)
        .map(|index| TaskState {
            task_id: format!("task-{index}"),
            status: TaskStatus::Done,
            turns: 0,
            last_at: index,
            turn_blocks: Vec::new(),
            attention: None,
            question_id: None,
            work: None,
        })
        .collect();

    let groups = app().split_fold(&[lane]);
    let group = groups.first().expect("the agent lane becomes one group");

    assert_eq!(group.visible_tasks, 10);
    assert_eq!(group.hidden, 1);
    assert!(group.overflow);
}

#[test]
fn paging_starts_with_the_fold_running_first_task_order() {
    let mut lane = shell_lane();
    lane.tasks = vec![
        TaskState {
            task_id: "completed-recently".into(),
            status: TaskStatus::Done,
            turns: 0,
            last_at: 900,
            turn_blocks: Vec::new(),
            attention: None,
            question_id: None,
            work: None,
        },
        TaskState {
            task_id: "running-older".into(),
            status: TaskStatus::Running,
            turns: 0,
            last_at: 100,
            turn_blocks: Vec::new(),
            attention: None,
            question_id: None,
            work: None,
        },
    ];

    let group = app()
        .split_fold(&[lane])
        .pop()
        .expect("the agent lane becomes one group");

    assert_eq!(
        group
            .sessions
            .iter()
            .filter_map(|session| session.task.as_ref().map(|task| task.task_id.as_str()))
            .collect::<Vec<_>>(),
        vec!["running-older", "completed-recently"],
        "the first task page must match the fold's running-first order"
    );
}

#[test]
fn paging_applies_name_sort_before_selecting_the_visible_tasks() {
    let lanes = vec![shell_lane()];
    let mut owner = group(vec![task_row("zulu"), task_row("alpha")]);
    owner.visible_tasks = 1;
    owner.hidden = 1;
    owner.overflow = true;
    sort_sessions(&mut owner.sessions, SidebarSort::Name);
    let mut rows = Vec::new();

    push_group(
        &mut rows,
        &mut owner,
        &medulla::control_socket::HarnessRunRegistry::new(),
        &lanes,
        None,
    );

    assert!(rows.iter().any(|row| matches!(
        row,
        RailRow::Session(session)
            if session.task.as_ref().is_some_and(|task| task.task_id == "alpha")
    )));
    assert!(!rows.iter().any(|row| matches!(
        row,
        RailRow::Session(session)
            if session.task.as_ref().is_some_and(|task| task.task_id == "zulu")
    )));
}

#[test]
fn paging_keeps_the_anchored_task_after_recent_sorting() {
    let lanes = vec![shell_lane()];
    let mut owner = group(vec![
        task_row("newest"),
        task_row("middle"),
        task_row("selected"),
    ]);
    owner.sessions[0].task.as_mut().expect("task row").last_at = 900;
    owner.sessions[1].task.as_mut().expect("task row").last_at = 500;
    owner.sessions[2].task.as_mut().expect("task row").last_at = 100;
    sort_sessions(&mut owner.sessions, SidebarSort::Recent);
    owner.visible_tasks = 1;
    owner.overflow = true;
    let mut rows = Vec::new();

    push_group(
        &mut rows,
        &mut owner,
        &medulla::control_socket::HarnessRunRegistry::new(),
        &lanes,
        Some(&RailAnchor::Task {
            lane: "agent:shell".into(),
            task_id: "selected".into(),
        }),
    );

    assert!(rows.iter().any(|row| matches!(
        row,
        RailRow::Session(session)
            if session.task.as_ref().is_some_and(|task| task.task_id == "selected")
    )));
}

#[test]
fn paging_keeps_a_task_backed_session_with_an_active_workflow_run() {
    let lanes = vec![shell_lane()];
    let mut active = task_row("active-workflow");
    let mut local = super::super::tests::stub_session("pty-1");
    local.mcp_grant_session = Some("grant-1".into());
    active.local = Some(local);
    let mut owner = group(vec![task_row("newest"), active]);
    owner.visible_tasks = 1;
    owner.overflow = true;
    let runs = medulla::control_socket::HarnessRunRegistry::new();
    runs.report(
        "grant-1",
        RunReport {
            run_id: "run-1".into(),
            workflow_id: "workflow".into(),
            status: HarnessRunStatus::Running,
            detail: None,
            node: None,
        },
    );
    let mut rows = Vec::new();

    push_group(&mut rows, &mut owner, &runs, &lanes, None);

    assert!(rows.iter().any(|row| matches!(
        row,
        RailRow::WorkflowRun(run) if run.run.run_id == "run-1"
    )));
}

#[test]
fn paging_hides_the_overflow_action_when_retention_shows_every_task() {
    let lanes = vec![shell_lane()];
    let mut owner = group(vec![task_row("first"), task_row("pinned")]);
    owner.visible_tasks = 1;
    owner.hidden = 1;
    owner.overflow = true;
    let mut rows = Vec::new();

    push_group(
        &mut rows,
        &mut owner,
        &medulla::control_socket::HarnessRunRegistry::new(),
        &lanes,
        Some(&RailAnchor::Task {
            lane: "agent:shell".into(),
            task_id: "pinned".into(),
        }),
    );

    assert!(
        !rows
            .iter()
            .any(|row| matches!(row, RailRow::Overflow { .. })),
        "the overflow action is absent when retaining a task reveals every task"
    );
}
