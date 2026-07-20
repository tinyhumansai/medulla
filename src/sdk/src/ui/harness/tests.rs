//! Unit tests for the harness view-model helpers: board summary/lines and the
//! read-only budget note.

use super::*;
use crate::harness_contract::{HarnessState, HarnessStatus, HarnessUsage, TrackedTask};

fn task(id: &str, title: &str, status: TrackedTaskStatus) -> TrackedTask {
    TrackedTask {
        id: id.into(),
        title: title.into(),
        detail: None,
        status,
        created_at: "2026-07-20T00:00:00.000Z".into(),
        updated_at: "2026-07-20T00:00:00.000Z".into(),
        instruction_id: None,
        delegated_task_ids: Vec::new(),
        notes: Vec::new(),
    }
}

fn status_with(tasks: Vec<TrackedTask>) -> HarnessStatus {
    HarnessStatus {
        state: HarnessState::Running,
        queued: 0,
        active_instruction_id: None,
        active_cycle_id: None,
        tasks,
        running_delegations: 0,
        usage: HarnessUsage::default(),
        last_result: None,
        escalations: Vec::new(),
    }
}

#[test]
fn summary_omits_zero_status_counts() {
    let tasks = vec![
        task("1", "a", TrackedTaskStatus::Open),
        task("2", "b", TrackedTaskStatus::Active),
        task("3", "c", TrackedTaskStatus::Active),
        task("4", "d", TrackedTaskStatus::Done),
    ];
    assert_eq!(task_board_summary(&tasks), "open 1 · active 2 · done 1");
}

#[test]
fn summary_and_lines_are_empty_for_empty_board() {
    assert_eq!(task_board_summary(&[]), "");
    assert!(task_board_lines(&status_with(vec![]), 40).is_empty());
}

#[test]
fn board_lines_have_a_header_plus_one_row_per_task() {
    let status = status_with(vec![
        task("1", "First thing", TrackedTaskStatus::Active),
        task("2", "Second thing", TrackedTaskStatus::Blocked),
    ]);
    let lines = task_board_lines(&status, 40);
    assert_eq!(lines.len(), 3); // header + 2 tasks
    assert!(lines[0].text.starts_with("tasks · "));
    assert!(lines[1].text.starts_with('●'));
    assert!(lines[2].text.starts_with('◑'));
}

#[test]
fn board_line_titles_are_ellipsized_to_width() {
    let status = status_with(vec![task(
        "1",
        "An extremely long task title that exceeds the pane",
        TrackedTaskStatus::Open,
    )]);
    let lines = task_board_lines(&status, 16);
    // glyph + space + clipped title, never wider than the requested columns.
    assert!(lines[1].text.chars().count() <= 16, "{}", lines[1].text);
    assert!(lines[1].text.contains('…'));
}

#[test]
fn every_status_maps_to_a_glyph_and_colour() {
    for status in [
        TrackedTaskStatus::Open,
        TrackedTaskStatus::Active,
        TrackedTaskStatus::Blocked,
        TrackedTaskStatus::Done,
        TrackedTaskStatus::Cancelled,
    ] {
        // Never panics; colour is one of the known palette names.
        let _ = task_glyph(status);
        assert!(!task_color(status).is_empty());
    }
}

fn budget(plan_label: &str, headroom: u64, exhausted: bool) -> AgentBudgetMetadata {
    AgentBudgetMetadata {
        seat_id: "seat-1".into(),
        provider: "anthropic".into(),
        plan: "claude_max_5x".into(),
        plan_label: plan_label.into(),
        headroom_tokens: headroom,
        exhausted,
        primary_resets_at: "2026-07-20T05:00:00.000Z".into(),
    }
}

#[test]
fn budget_note_scales_headroom_into_millions() {
    assert_eq!(
        budget_note(&budget("Claude Max 5×", 1_250_000, false)),
        "Claude Max 5× · 1.2M left"
    );
    assert_eq!(
        budget_note(&budget("Claude Pro", 42_000, false)),
        "Claude Pro · 42k left"
    );
    assert_eq!(
        budget_note(&budget("ChatGPT Plus", 900, false)),
        "ChatGPT Plus · 900 left"
    );
}

#[test]
fn budget_note_reads_exhausted_when_seat_is_spent() {
    assert_eq!(
        budget_note(&budget("Claude Max 5×", 0, true)),
        "Claude Max 5× · exhausted"
    );
}
