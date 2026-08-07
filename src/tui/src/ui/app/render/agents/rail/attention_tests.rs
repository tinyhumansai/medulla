//! Tests for attention cues in session rows and lane state classification.

use medulla::ui::agents::{AgentLane, AgentRole, TaskState, TaskStatus};
use ratatui::style::{Color, Modifier};

use crate::worker::pty::{AttentionKind, HarnessAttention, PtyState, SessionRow};

use super::rail_title;
use super::state::{classify_lane, lane_waiting_session, task_waiting_session};
use super::status::HarnessVisualState;
use super::tests::{app, harness_row, NOW};
use crate::ui::app::rail::RailRow;

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
    let lines = app.own_session_lines(&waiting_row("/workspace/medulla"), false, 48, NOW);

    assert!(lines[0].to_string().starts_with("⚠ codex"), "{}", lines[0]);
    let style = lines[0].spans[0].style;
    assert_eq!(style.fg, Some(Color::Yellow));
    assert!(style.add_modifier.contains(Modifier::BOLD));
    assert!(lines[1].to_string().contains("asking permission"));
}

/// The pulse is driven off the frame counter rather than `SLOW_BLINK`, so it has
/// to be observable by stepping frames — which is also the only way it is
/// observable to an operator whose terminal ignores the modifier, which is most
/// of them.
#[test]
fn a_waiting_harness_alternates_bright_and_dim_across_frames() {
    let mut app = app();
    let row = waiting_row("/workspace/medulla");
    let half = (app.theme.attention_blink_ms / 2) / crate::ui::theme::FRAME_MS;

    let bright = app.own_session_lines(&row, false, 48, NOW)[0].spans[0].style;
    app.frame = half as usize;
    let dim = app.own_session_lines(&row, false, 48, NOW)[0].spans[0].style;
    app.frame = (half * 2) as usize;
    let bright_again = app.own_session_lines(&row, false, 48, NOW)[0].spans[0].style;

    assert!(bright.add_modifier.contains(Modifier::BOLD));
    assert!(dim.add_modifier.contains(Modifier::DIM));
    assert!(bright_again.add_modifier.contains(Modifier::BOLD));
}

/// A slower configured pulse holds each phase for proportionally longer. Without
/// this the rate control could be persisted and read back while changing nothing
/// anyone can see.
#[test]
fn the_configured_rate_sets_how_long_a_phase_lasts() {
    let mut app = app();
    let row = waiting_row("/workspace/medulla");
    app.theme.attention_blink_ms = 4_000;
    // Where a one-second pulse would already have gone dim.
    app.frame = ((1_000 / 2) / crate::ui::theme::FRAME_MS) as usize;

    let style = app.own_session_lines(&row, false, 48, NOW)[0].spans[0].style;

    assert!(
        style.add_modifier.contains(Modifier::BOLD),
        "a four-second pulse is still bright half a second in"
    );
}

#[test]
fn blinking_switched_off_holds_the_cue_bright() {
    let mut app = app();
    let row = waiting_row("/workspace/medulla");
    app.theme.attention_blink = false;
    app.frame = 7;

    let style = app.own_session_lines(&row, false, 48, NOW)[0].spans[0].style;

    assert_eq!(style.fg, Some(Color::Yellow));
    assert!(style.add_modifier.contains(Modifier::BOLD));
    assert!(!style.add_modifier.contains(Modifier::DIM));
}

#[test]
fn a_selected_harness_waiting_on_you_stays_blinking_yellow() {
    let app = app();
    let lines = app.own_session_lines(&waiting_row("/workspace/medulla"), true, 48, NOW);
    let style = lines[0].spans[0].style;

    assert_eq!(style.fg, Some(Color::Yellow));
    assert!(style.add_modifier.contains(Modifier::BOLD));
    assert!(style.add_modifier.contains(Modifier::REVERSED));
}

/// The two states that pulse must not pulse the same colour: one is a question
/// and the other is a failure, and an operator triaging a rail needs to know
/// which before reading any words.
#[test]
fn a_failed_harness_pulses_red_and_says_why() {
    let app = app();
    let mut row = harness_row("/workspace/medulla");
    row.state = PtyState::Exited { code: Some(2) };

    let lines = app.own_session_lines(&row, false, 48, NOW);

    assert!(lines[0].to_string().starts_with("✕ codex"), "{}", lines[0]);
    assert_eq!(lines[0].spans[0].style.fg, Some(Color::Red));
    assert!(lines
        .iter()
        .any(|line| line.to_string().contains("exited with 2")));
}

/// A write that never reached the child is the most specific thing we know about
/// why a session is dead, so it wins over the exit status.
#[test]
fn a_recorded_error_is_reported_over_the_exit_status() {
    let app = app();
    let mut row = harness_row("/workspace/medulla");
    row.state = PtyState::Failed;
    row.last_error = Some("pty closed".into());

    let rendered = app
        .own_session_lines(&row, false, 48, NOW)
        .iter()
        .map(|line| line.to_string())
        .collect::<Vec<_>>()
        .join(" ");

    assert!(rendered.contains("pty closed"), "{rendered}");
}

/// A dispatched turn that settled leaves the session standing for someone to
/// read. Nothing else on the row says so — it is running, unbusy, and idle at a
/// composer, exactly like a session nobody has used yet.
#[test]
fn a_retained_session_says_its_work_is_finished() {
    let app = app();
    let mut row = harness_row("/workspace/medulla");
    row.retained = true;

    let lines = app.own_session_lines(&row, false, 48, NOW);

    assert!(lines[0].to_string().starts_with("✓ codex"), "{}", lines[0]);
    assert!(lines
        .iter()
        .any(|line| line.to_string().contains("finished")));
}

/// A live harness mid-turn spins. Every running session drew the same dot
/// before, so the rail could say a harness was alive but never that it was busy.
#[test]
fn a_working_harness_spins_through_the_frames() {
    let mut app = app();
    let mut row = harness_row("/workspace/medulla");
    row.working = true;

    let first = app.own_session_lines(&row, false, 48, NOW)[0].to_string();
    app.frame = 1;
    let second = app.own_session_lines(&row, false, 48, NOW)[0].to_string();

    assert_ne!(first, second, "the working glyph must advance with the frame");
    assert!(
        crate::ui::util::SPINNER
            .iter()
            .any(|frame| first.starts_with(frame)),
        "{first}"
    );
}

/// The complement: a live session nobody is asking anything of stays still.
/// An animation on every row is the same as an animation on none.
#[test]
fn an_idle_harness_does_not_spin() {
    let mut app = app();
    let row = harness_row("/workspace/medulla");

    let first = app.own_session_lines(&row, false, 48, NOW)[0].to_string();
    app.frame = 5;
    let second = app.own_session_lines(&row, false, 48, NOW)[0].to_string();

    assert_eq!(first, second);
    assert!(first.starts_with('○'), "{first}");
}

#[test]
fn a_long_attention_reason_wraps_without_losing_words() {
    let app = app();
    let mut row = waiting_row("/workspace/medulla");
    let reason = "claude is waiting for you to accept the bypass-permissions disclaimer";
    row.attention = Some(HarnessAttention::new(AttentionKind::Dialog, reason, NOW));

    let rendered = app
        .own_session_lines(&row, false, 36, NOW)
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

    let lines = app.own_session_lines(&row, false, 48, NOW);

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

    let lines = app.own_session_lines(&row, false, 48, NOW);

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
fn rail_title_counts_the_agents_on_the_tree_not_the_lanes() {
    // Two agents on the rail and one of them running a task. Counting lanes
    // instead would report the agents that happen to have traffic — one — and
    // read as "Agents · 1" on a machine that has two.
    let mut item = lane();
    item.tasks = vec![task(TaskStatus::Running, 1)];
    let rows = vec![
        RailRow::Agent(agent_row("busy")),
        RailRow::Agent(agent_row("idle")),
        RailRow::NewAgent,
    ];

    assert_eq!(
        rail_title(&rows, &[item], 2),
        "Agents · 2 · 1 running · ⚠ 2 waiting on you"
    );
}

/// A bare agent row, as the tree produces one for a declared agent.
fn agent_row(agent_id: &str) -> crate::ui::app::rail::AgentRailRow {
    crate::ui::app::rail::AgentRailRow {
        agent_id: agent_id.to_string(),
        host_id: String::new(),
        agent: None,
        lane_index: None,
    }
}
