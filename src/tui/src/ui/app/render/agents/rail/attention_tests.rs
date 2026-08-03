//! Tests for attention cues in harness rows and lane state classification.

use medulla::ui::agents::{AgentLane, AgentRole, TaskState, TaskStatus};
use ratatui::style::{Color, Modifier};

use crate::worker::pty::{AttentionKind, HarnessAttention, PtyState, SessionRow};

use super::rail_title;
use super::state::{classify_lane, lane_waiting_session, task_waiting_session};
use super::status::HarnessVisualState;
use super::tests::{app, harness_row, NOW};

fn lane() -> AgentLane {
    AgentLane {
        key: "k".into(),
        label: "worker".into(),
        role: AgentRole::Agent,
        turns: Vec::new(),
        last_at: 0,
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

fn task(status: TaskStatus, at: i64) -> TaskState {
    TaskState {
        task_id: format!("task-{at}"),
        status,
        turns: 1,
        last_at: at,
        turn_blocks: Vec::new(),
        attention: None,
        question_id: None,
        work: None,
    }
}

/// A harness row carrying a cue, as the pty layer would have set it.
fn waiting_row(cwd: &str) -> SessionRow {
    let mut row = harness_row(cwd);
    row.attention = Some(HarnessAttention::new(
        AttentionKind::Approval,
        "codex is asking permission",
        0,
    ));
    row
}

#[test]
fn a_harness_waiting_on_you_blinks_and_says_what_it_wants() {
    let app = app();
    let lines = app.own_harness_lines(&waiting_row("/workspace/medulla"), false, 48, NOW);

    assert!(lines[0].to_string().starts_with("⚠ codex"), "{}", lines[0]);
    let style = lines[0].spans[0].style;
    assert_eq!(style.fg, Some(Color::Yellow));
    assert!(style.add_modifier.contains(Modifier::SLOW_BLINK));
    assert!(lines[1].to_string().contains("asking permission"));
}

#[test]
fn a_selected_harness_waiting_on_you_stays_blinking_yellow() {
    let app = app();
    let lines = app.own_harness_lines(&waiting_row("/workspace/medulla"), true, 48, NOW);
    let style = lines[0].spans[0].style;

    assert_eq!(style.fg, Some(Color::Yellow));
    assert!(style.add_modifier.contains(Modifier::SLOW_BLINK));
    assert!(style.add_modifier.contains(Modifier::REVERSED));
}

#[test]
fn attention_uses_the_configured_color_and_can_stay_solid() {
    let mut app = app();
    app.theme.attention = Color::LightMagenta;
    app.theme.attention_blink = false;

    let lines = app.own_harness_lines(&waiting_row("/workspace/medulla"), false, 48, NOW);
    let style = lines[0].spans[0].style;

    assert_eq!(style.fg, Some(Color::LightMagenta));
    assert!(!style.add_modifier.contains(Modifier::SLOW_BLINK));
}

#[test]
fn a_long_attention_reason_wraps_without_losing_words() {
    let app = app();
    let mut row = waiting_row("/workspace/medulla");
    let reason = "claude is waiting for you to accept the bypass-permissions disclaimer";
    row.attention = Some(HarnessAttention::new(AttentionKind::Dialog, reason, NOW));

    let rendered = app
        .own_harness_lines(&row, false, 36, NOW)
        .iter()
        .skip(1)
        .map(|line| line.to_string().trim().to_string())
        .collect::<Vec<_>>()
        .join(" ");
    assert!(
        rendered.contains(reason),
        "complete reason was {rendered:?}"
    );
}

#[test]
fn the_pane_you_are_typing_in_does_not_blink_at_you() {
    let mut app = app();
    let row = waiting_row("/workspace/medulla");
    app.harness_focus = crate::ui::harness_pane::HarnessFocus::Attached(row.id.clone());

    let lines = app.own_harness_lines(&row, false, 48, NOW);

    assert_eq!(lines.len(), 1, "no second line: {lines:?}");
    assert!(!lines[0].spans[0]
        .style
        .add_modifier
        .contains(Modifier::SLOW_BLINK));
}

#[test]
fn an_exited_harness_stops_asking_for_anything() {
    let app = app();
    let mut row = waiting_row("/workspace/medulla");
    row.state = PtyState::Exited { code: Some(0) };

    let lines = app.own_harness_lines(&row, false, 48, NOW);

    assert_eq!(lines.len(), 1);
    assert!(lines[0].to_string().starts_with("✓ codex"), "{}", lines[0]);
}

#[test]
fn lane_attention_skips_the_attached_session_and_takes_the_first_waiting_one() {
    let mut item = lane();
    item.tasks = vec![
        task(TaskStatus::Running, 1),
        task(TaskStatus::Running, 2),
        task(TaskStatus::Running, 3),
    ];
    let waiting = ["w-1".to_string(), "w-2".to_string()].into_iter().collect();

    let found = lane_waiting_session(&item, Some("w-1"), &waiting, |task_id| match task_id {
        "task-1" => Some("w-1".to_string()),
        "task-2" => Some("not-waiting".to_string()),
        "task-3" => Some("w-2".to_string()),
        _ => None,
    });

    assert_eq!(found.as_deref(), Some("w-2"));
    assert_eq!(
        classify_lane(&item, Some("running"), found.is_some()),
        HarnessVisualState::NeedsInput,
        "PTY attention must outrank ordinary running state"
    );
}

#[test]
fn task_attention_marks_only_the_exact_waiting_session() {
    let waiting = ["w-2".to_string()].into_iter().collect();
    let resolve = |task_id: &str| match task_id {
        "task-1" => Some("w-1".to_string()),
        "task-2" => Some("w-2".to_string()),
        _ => None,
    };

    assert_eq!(
        task_waiting_session("task-2", None, &waiting, resolve).as_deref(),
        Some("w-2")
    );
    assert_eq!(
        task_waiting_session("task-1", None, &waiting, resolve),
        None
    );
    assert_eq!(
        task_waiting_session("task-2", Some("w-2"), &waiting, resolve),
        None,
        "the attached pane is already being handled"
    );
}

#[test]
fn rail_title_reports_the_attention_snapshot_count() {
    let mut item = lane();
    item.tasks = vec![task(TaskStatus::Running, 1)];

    assert_eq!(
        rail_title(&[item], 2),
        "Agents · 1 · 1 running · ⚠ 2 waiting on you"
    );
}
