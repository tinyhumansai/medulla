//! Unit tests for the work panel's rendering and its one-line summaries.

use super::*;
use crate::harness_work::{kinds, WorkFold, WorkSnapshot};
use serde_json::json;

fn snapshot(events: &[(&str, serde_json::Value)]) -> WorkSnapshot {
    let mut fold = WorkFold::new();
    for (i, (kind, payload)) in events.iter().enumerate() {
        fold.apply(kind, payload, 1_000 + i as i64);
    }
    fold.into_snapshot()
}

fn todos(items: &[(&str, &str)]) -> serde_json::Value {
    json!({ "todos": items.iter().map(|(text, status)| json!({ "content": text, "status": status })).collect::<Vec<_>>() })
}

fn text_of(lines: &[crate::ui::agents::Line]) -> Vec<String> {
    lines.iter().map(|line| line.text.clone()).collect()
}

#[test]
fn an_empty_snapshot_renders_no_panel_at_all() {
    assert!(work_lines(&WorkSnapshot::default(), 40).is_empty());
}

#[test]
fn the_panel_shows_the_todo_list_with_its_progress() {
    let work = snapshot(&[(
        kinds::TODO_UPDATE,
        todos(&[("read it", "completed"), ("write it", "in_progress")]),
    )]);
    let lines = text_of(&work_lines(&work, 40));
    assert!(lines.contains(&"tasks · 1/2 done".to_string()));
    assert!(lines.iter().any(|l| l.contains("☑ read it")));
    assert!(lines.iter().any(|l| l.contains("▶ write it")));
}

#[test]
fn a_long_todo_list_keeps_the_unfinished_items() {
    let mut items: Vec<(&str, &str)> = (0..20).map(|_| ("done thing", "completed")).collect();
    items.push(("the live one", "in_progress"));
    let work = snapshot(&[(kinds::TODO_UPDATE, todos(&items))]);
    let lines = text_of(&work_lines(&work, 40));
    assert!(
        lines.iter().any(|l| l.contains("the live one")),
        "truncation must not drop the item still in play"
    );
    assert!(lines.iter().any(|l| l.starts_with("… +")));
}

#[test]
fn sub_agents_render_with_their_type_and_running_ones_come_first() {
    let work = snapshot(&[
        (
            kinds::SUBAGENT_START,
            json!({ "call_id": "a", "description": "finished job" }),
        ),
        ("tool_result", json!({ "call_id": "a", "is_error": false })),
        (
            kinds::SUBAGENT_START,
            json!({ "call_id": "b", "description": "live job", "agent_type": "code-reviewer" }),
        ),
    ]);
    let lines = text_of(&work_lines(&work, 60));
    let heading = lines
        .iter()
        .position(|l| l.starts_with("sub-agents"))
        .expect("the section renders");
    assert_eq!(lines[heading], "sub-agents · 2 · 1 running");
    assert!(lines[heading + 1].contains("live job (code-reviewer)"));
    assert!(lines[heading + 2].contains("finished job"));
}

#[test]
fn files_are_counted_rather_than_repeated() {
    let work = snapshot(&[
        (
            kinds::FILE_CHANGE,
            json!({ "path": "src/a.rs", "change": "modified" }),
        ),
        (
            kinds::FILE_CHANGE,
            json!({ "path": "src/a.rs", "change": "modified" }),
        ),
    ]);
    let lines = text_of(&work_lines(&work, 40));
    assert!(lines.contains(&"files · 1 touched".to_string()));
    assert!(lines.iter().any(|l| l.contains("src/a.rs ×2")));
}

#[test]
fn a_plan_that_repeats_the_todo_list_is_not_drawn_twice() {
    let work = snapshot(&[
        (
            kinds::PLAN_UPDATE,
            json!({ "steps": [{ "step": "one" }, { "step": "two" }] }),
        ),
        (
            kinds::TODO_UPDATE,
            todos(&[("one", "completed"), ("two", "pending")]),
        ),
    ]);
    let lines = text_of(&work_lines(&work, 40));
    assert!(!lines.iter().any(|l| l.starts_with("plan ·")));
    assert_eq!(lines.iter().filter(|l| l.ends_with(" one")).count(), 1);
}

#[test]
fn a_distinct_plan_renders_beside_the_todos() {
    let work = snapshot(&[
        (
            kinds::PLAN_UPDATE,
            json!({ "goal": "make it legible", "steps": [{ "step": "survey" }] }),
        ),
        (kinds::TODO_UPDATE, todos(&[("write the fold", "pending")])),
    ]);
    let lines = text_of(&work_lines(&work, 40));
    assert!(lines.contains(&"goal".to_string()));
    assert!(lines.iter().any(|l| l.contains("make it legible")));
    assert!(lines.contains(&"plan · 1 step(s)".to_string()));
    assert!(lines.contains(&"tasks · 0/1 done".to_string()));
}

#[test]
fn the_session_and_outcome_sections_report_what_was_measured() {
    let work = snapshot(&[
        (
            kinds::SESSION_INFO,
            json!({ "model": "claude-opus-5", "cwd": "/repo", "tools": ["Bash", "Read"] }),
        ),
        (
            kinds::RUN_RESULT,
            json!({ "ok": true, "num_turns": 4, "duration_ms": 8100, "cost_usd": 0.19 }),
        ),
    ]);
    let lines = text_of(&work_lines(&work, 60));
    assert!(lines.iter().any(|l| l.contains("claude-opus-5")));
    assert!(lines.iter().any(|l| l.contains("2 tools")));
    assert!(lines.iter().any(|l| l.contains("/repo")));
    assert!(lines
        .iter()
        .any(|l| l.contains("ok · 4 turn(s) · 8.1s · $0.19")));
}

#[test]
fn the_chip_is_short_enough_for_a_rail_row() {
    let work = snapshot(&[
        (
            kinds::TODO_UPDATE,
            todos(&[("a", "completed"), ("b", "in_progress")]),
        ),
        (
            kinds::SUBAGENT_START,
            json!({ "call_id": "s1", "description": "d" }),
        ),
    ]);
    assert_eq!(work_chip(&work), "1/2 ⑂1");
    assert!(work_chip(&WorkSnapshot::default()).is_empty());
}

#[test]
fn the_headline_prefers_what_the_agent_is_on_right_now() {
    let work = snapshot(&[
        (kinds::PLAN_UPDATE, json!({ "goal": "the big goal" })),
        (
            kinds::TODO_UPDATE,
            todos(&[("a", "completed"), ("b", "in_progress")]),
        ),
    ]);
    assert_eq!(work_headline(&work).as_deref(), Some("b · 1/2"));

    // With nothing in progress the goal is the next best answer.
    let goal_only = snapshot(&[(kinds::PLAN_UPDATE, json!({ "goal": "the big goal" }))]);
    assert_eq!(work_headline(&goal_only).as_deref(), Some("the big goal"));
    assert_eq!(work_headline(&WorkSnapshot::default()), None);
}
