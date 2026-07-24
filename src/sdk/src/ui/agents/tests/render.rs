//! Tests for status/role classification, key parsing, task ordering, and
//! transcript rendering (plus the shared formatting helpers).

use super::env;
use crate::runtime::AgentDescriptor;
use crate::ui::agents::*;
use crate::ui::events::TuiEvent;

// The formatting helpers are private to the `agents` module; reach them through
// the module ancestor rather than the public re-exports.
use super::super::fmt::{event_kind_color, tool_line};

#[test]
fn task_status_from_wire_maps_all_states() {
    assert_eq!(TaskStatus::from_wire("done"), TaskStatus::Done);
    assert_eq!(TaskStatus::from_wire("cancelled"), TaskStatus::Cancelled);
    assert_eq!(TaskStatus::from_wire("failed"), TaskStatus::Failed);
    // Any unrecognized status is failed, never silently "done".
    assert_eq!(TaskStatus::from_wire("weird"), TaskStatus::Failed);
}

#[test]
fn task_status_labels_and_colors() {
    for (s, label, color) in [
        (TaskStatus::Running, "running", "yellow"),
        (TaskStatus::Done, "done", "green"),
        (TaskStatus::Failed, "failed", "red"),
        (TaskStatus::Cancelled, "cancelled", "gray"),
    ] {
        assert_eq!(s.label(), label);
        assert_eq!(s.color(), color);
    }
}

#[test]
fn agent_role_color_and_function() {
    assert_eq!(AgentRole::Orchestrator.color(), "yellow");
    assert_eq!(AgentRole::Reasoning.color(), "yellow");
    assert_eq!(AgentRole::Compress.color(), "blue");
    assert_eq!(AgentRole::Worker.color(), "magenta");
    assert!(AgentRole::Compress.is_function());
    assert!(!AgentRole::Worker.is_function());
    assert!(!AgentRole::Orchestrator.is_function());
}

#[test]
fn parse_task_key_splits_cycle_and_bare() {
    assert_eq!(parse_task_key("cyc-1/t:task-9"), (Some("cyc-1"), "task-9"));
    assert_eq!(parse_task_key("task-9"), (None, "task-9"));
}

#[test]
fn ordered_tasks_puts_running_first_then_recency() {
    let mk = |id: &str, status: TaskStatus, at: i64| TaskState {
        task_id: id.into(),
        instruction: None,
        status,
        turns: 0,
        last_at: at,
        turn_blocks: Vec::new(),
        attention: None,
        question_id: None,
        review: None,
    };
    let tasks = vec![
        mk("done-old", TaskStatus::Done, 10),
        mk("run-old", TaskStatus::Running, 20),
        mk("done-new", TaskStatus::Done, 30),
        mk("run-new", TaskStatus::Running, 40),
    ];
    let ordered = ordered_tasks(&tasks);
    let ids: Vec<&str> = ordered.iter().map(|t| t.task_id.as_str()).collect();
    // Running first (newest→oldest), then non-running (newest→oldest).
    assert_eq!(ids, vec!["run-new", "run-old", "done-new", "done-old"]);
}

#[test]
fn lane_lines_none_and_empty_and_flat() {
    assert!(lane_lines(None, 40).is_empty());
    // A tier lane with no turns renders the "No turns yet." placeholder.
    let lanes = derive_agent_lanes(&[], "", &[]);
    let lines = lane_lines(Some(&lanes[0]), 40);
    assert_eq!(lines.len(), 1);
    assert!(lines[0].text.contains("No turns yet"));
}

#[test]
fn lane_lines_groups_agent_tasks_with_headers() {
    let roster = vec![AgentDescriptor {
        id: "dev".into(),
        name: "Dev".into(),
        description: String::new(),
        availability: "online".into(),
        tags: vec![],
        metadata: serde_json::Map::new(),
    }];
    let events = vec![env(
        1,
        TuiEvent::TaskStart {
            task_id: "t1".into(),
            instruction: "do the thing".into(),
            depth: 2,
            agent_id: Some("dev".into()),
            contract: None,
        },
    )];
    let lanes = derive_agent_lanes(&events, "TINYPLACE", &roster);
    let dev = lanes.iter().find(|l| l.key == "agent:dev").unwrap();
    let lines = lane_lines(Some(dev), 60);
    // A per-task header divider precedes the turn body.
    assert!(lines.iter().any(|l| l.text.contains("── t1 · running")));
}

#[test]
fn task_lines_empty_and_populated() {
    let empty = TaskState {
        task_id: "t1".into(),
        instruction: None,
        status: TaskStatus::Running,
        turns: 0,
        last_at: 0,
        turn_blocks: Vec::new(),
        attention: None,
        question_id: None,
        review: None,
    };
    let lines = task_lines(&empty, 40);
    assert_eq!(lines.len(), 1);
    assert!(lines[0].text.contains("No turns yet"));

    let mut task = empty;
    task.turn_blocks.push(TurnBlock {
        at: 1000,
        header: "text".into(),
        header_color: Some("green".into()),
        reasoning: Some("thinking hard".into()),
        content: Some("the output".into()),
        tools: vec!["→ grep({})".into()],
    });
    let lines = task_lines(&task, 60);
    // Header, thinking, output, and tools sections all render.
    let joined: String = lines
        .iter()
        .map(|l| l.text.clone())
        .collect::<Vec<_>>()
        .join("|");
    assert!(joined.contains("thinking"));
    assert!(joined.contains("output"));
    assert!(joined.contains("tools"));
}

#[test]
fn lane_lines_agent_task_with_no_turns_shows_placeholder() {
    // A worker agent lane whose task has folded no turns renders a per-task
    // header followed by the "(no turns yet)" placeholder.
    let lane = AgentLane {
        key: "agent:dev".into(),
        label: "Dev".into(),
        role: AgentRole::Worker,
        turns: Vec::new(),
        last_at: 0,
        tasks: vec![TaskState {
            task_id: "t1".into(),
            instruction: None,
            status: TaskStatus::Running,
            turns: 0,
            last_at: 0,
            turn_blocks: Vec::new(),
            attention: None,
            question_id: None,
            review: None,
        }],
        context_tokens: None,
        harness_label: None,
        agent_id: Some("dev".into()),
        session_id: None,
        parent_agent_id: None,
        descriptor: None,
        active_tasks: 1,
    };
    let lines = lane_lines(Some(&lane), 60);
    let joined: String = lines
        .iter()
        .map(|l| l.text.clone())
        .collect::<Vec<_>>()
        .join("|");
    assert!(joined.contains("── t1 · running"));
    assert!(joined.contains("(no turns yet)"));
}

#[test]
fn task_lines_truncate_long_header_and_default_color() {
    // An over-wide header is truncated with an ellipsis; a colour-less header
    // falls back to cyan.
    let task = TaskState {
        task_id: "t".into(),
        instruction: None,
        status: TaskStatus::Running,
        turns: 1,
        last_at: 0,
        turn_blocks: vec![TurnBlock {
            at: 1000,
            header: "H".repeat(200),
            header_color: None,
            reasoning: None,
            content: None,
            tools: Vec::new(),
        }],
        attention: None,
        question_id: None,
        review: None,
    };
    let lines = task_lines(&task, 30);
    assert!(lines[0].text.ends_with('…'));
    assert!(lines[0].text.chars().count() <= 30);
    assert_eq!(lines[0].color.as_deref(), Some("cyan"));
}

#[test]
fn tool_line_truncates_long_args() {
    let big = serde_json::json!({ "blob": "x".repeat(500) });
    let line = tool_line("write", &big);
    assert!(line.starts_with("→ write("));
    assert!(line.ends_with(')'));
    assert!(line.contains('…'), "long args should be ellipsized");
    assert!(line.chars().count() <= 220);
}

#[test]
fn event_kind_color_maps_known_kinds() {
    assert_eq!(event_kind_color("tool"), Some("blue"));
    assert_eq!(event_kind_color("prompt"), Some("cyan"));
    assert_eq!(event_kind_color("stdout"), Some("gray"));
    assert_eq!(event_kind_color("stderr"), Some("red"));
    assert_eq!(event_kind_color("error"), Some("red"));
    assert_eq!(event_kind_color("text"), Some("green"));
    assert_eq!(event_kind_color("thinking"), Some("yellow"));
    assert_eq!(event_kind_color("mystery"), None);
}
