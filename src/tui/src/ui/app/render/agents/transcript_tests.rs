//! Rendering regressions for session context in the transcript header.

use std::sync::Arc;

use medulla::config::LoadedConfig;
use medulla::harness_work::{HarnessSessionInfo, WorkSnapshot};
use medulla::runtime::mock::MockRuntime;
use medulla::runtime::Runtime;
use medulla::ui::agents::{AgentLane, AgentRole};
use ratatui::backend::TestBackend;
use ratatui::layout::Rect;
use ratatui::Terminal;

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
        lane_index: 0,
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
