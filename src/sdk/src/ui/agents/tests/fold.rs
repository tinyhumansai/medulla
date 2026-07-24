//! Tests for the event fold and the Agents-list row model.

use super::env;
use crate::runtime::AgentDescriptor;
use crate::ui::agents::*;
use crate::ui::events::{TaskDigest, TuiEvent, Usage};

fn agent(id: &str, name: &str) -> AgentDescriptor {
    AgentDescriptor {
        id: id.into(),
        name: name.into(),
        availability: "online".into(),
        ..Default::default()
    }
}

#[test]
fn tier_lanes_always_present_in_order() {
    let lanes = derive_agent_lanes(&[], "OPENCODE", &[]);
    // orchestrator, reasoning first; summarizer (function) last.
    assert_eq!(lanes.len(), 3);
    assert_eq!(lanes[0].label, "orchestrator");
    assert_eq!(lanes[1].label, "reasoning");
    assert_eq!(lanes[2].label, "summarizer");
    assert!(lanes[2].role.is_function());
}

#[test]
fn inference_end_folds_into_tier() {
    let events = vec![env(
        1,
        TuiEvent::InferenceEnd {
            tier: "reasoning".into(),
            op: "execute_step".into(),
            model: Some("gpt".into()),
            duration_ms: 42,
            usage: Some(Usage {
                input_tokens: 100,
                output_tokens: 20,
            }),
            content: Some("hi".into()),
            reasoning: None,
            tool_calls: None,
        },
    )];
    let lanes = derive_agent_lanes(&events, "", &[]);
    let reasoning = &lanes[1];
    assert_eq!(reasoning.turns.len(), 1);
    assert!(reasoning.turns[0]
        .header
        .contains("execute_step · gpt · 42ms"));
    assert_eq!(reasoning.context_tokens, Some(100));
}

#[test]
fn anonymous_task_lane_and_completion() {
    let events = vec![
        env(
            1,
            TuiEvent::TaskStart {
                task_id: "t1".into(),
                instruction: "do the thing".into(),
                depth: 2,
                agent_id: None,
            },
        ),
        env(
            2,
            TuiEvent::TaskEvent {
                task_id: "t1".into(),
                event_kind: "text".into(),
                content: "progress".into(),
                harness: None,
            },
        ),
        env(
            3,
            TuiEvent::TaskComplete {
                digest: TaskDigest {
                    task_id: "t1".into(),
                    status: "done".into(),
                    digest: "result".into(),
                    result_ref: None,
                    usage: Some(Usage {
                        input_tokens: 500,
                        output_tokens: 50,
                    }),
                    depth: 2,
                },
            },
        ),
    ];
    let lanes = derive_agent_lanes(&events, "OPENCODE", &[]);
    // orchestrator, reasoning, worker(t1), summarizer.
    let worker = lanes.iter().find(|l| l.key == "worker:t1").unwrap();
    assert_eq!(worker.label, "[OPENCODE] do the thing");
    assert_eq!(worker.active_tasks, 0);
    assert_eq!(worker.context_tokens, Some(500));
    assert_eq!(worker.tasks[0].status, TaskStatus::Done);
}

#[test]
fn agent_lane_stacks_tasks_with_row_model() {
    let roster = vec![AgentDescriptor {
        id: "dev".into(),
        name: "Dev".into(),
        description: String::new(),
        availability: "online".into(),
        tags: vec![],
        metadata: serde_json::Map::new(),
    }];
    let mut events = Vec::new();
    for i in 0..10 {
        events.push(env(
            i,
            TuiEvent::TaskStart {
                task_id: format!("t{i}"),
                instruction: "x".into(),
                depth: 2,
                agent_id: Some("dev".into()),
            },
        ));
    }
    let lanes = derive_agent_lanes(&events, "TINYPLACE", &roster);
    let dev = lanes.iter().find(|l| l.key == "agent:dev").unwrap();
    assert_eq!(dev.tasks.len(), 10);
    let rows = agent_row_model(&lanes, 8);
    // Cap at 8 sublanes + a "+2 more" row for the dev lane.
    let subs = rows
        .iter()
        .filter(|r| matches!(r, AgentRow::Sub { .. }))
        .count();
    let more = rows
        .iter()
        .filter(|r| matches!(r, AgentRow::More { .. }))
        .count();
    assert_eq!(subs, 8);
    assert_eq!(more, 1);
    // The functions divider precedes the summarizer.
    assert!(rows.iter().any(|r| matches!(r, AgentRow::Separator)));
}

#[test]
fn session_lanes_group_under_machine() {
    let events = vec![env(
        1,
        TuiEvent::PeerSession {
            agent_id: "m1".into(),
            session_id: "s1".into(),
            state: "working".into(),
            harness: Some("codex".into()),
        },
    )];
    let lanes = derive_agent_lanes(&events, "TINYPLACE", &[]);
    let session = lanes
        .iter()
        .find(|l| l.session_id.as_deref() == Some("s1"))
        .unwrap();
    assert_eq!(session.parent_agent_id.as_deref(), Some("m1"));
    // A session lane is tagged only with a harness it learned itself (CODEX),
    // never the global default (TINYPLACE).
    assert_eq!(session.harness_label.as_deref(), Some("CODEX"));
    assert_eq!(session.label, "[CODEX] ↳ s1");
}

#[test]
fn task_attention_sets_question_and_completion_clears_it() {
    let events = vec![
        env(
            1,
            TuiEvent::TaskStart {
                task_id: "t1".into(),
                instruction: "work".into(),
                depth: 2,
                agent_id: None,
            },
        ),
        env(
            2,
            TuiEvent::TaskAttention {
                task_id: "t1".into(),
                reason: "confirm".into(),
                content: "proceed?".into(),
                question_id: Some("q9".into()),
            },
        ),
    ];
    let lanes = derive_agent_lanes(&events, "OPENCODE", &[]);
    let worker = lanes.iter().find(|l| l.key == "worker:t1").unwrap();
    assert_eq!(
        worker.tasks[0].attention.as_deref(),
        Some("confirm: proceed?")
    );
    assert_eq!(worker.tasks[0].question_id.as_deref(), Some("q9"));

    // Completing the task clears the pending question and attention.
    let mut events = events;
    events.push(env(
        3,
        TuiEvent::TaskComplete {
            digest: TaskDigest {
                task_id: "t1".into(),
                status: "cancelled".into(),
                digest: String::new(),
                result_ref: None,
                usage: None,
                depth: 2,
            },
        },
    ));
    let lanes = derive_agent_lanes(&events, "OPENCODE", &[]);
    let worker = lanes.iter().find(|l| l.key == "worker:t1").unwrap();
    assert_eq!(worker.tasks[0].status, TaskStatus::Cancelled);
    assert!(worker.tasks[0].attention.is_none());
    assert!(worker.tasks[0].question_id.is_none());
}

#[test]
fn fresh_review_verdict_is_attributed_to_the_implementation_task() {
    let roster = vec![agent("dev-1", "Implementer"), agent("dev-2", "Reviewer")];
    let events = vec![
        env(
            1,
            TuiEvent::TaskStart {
                task_id: "task-1".into(),
                instruction: "Outcome: fix it\nVerify:\n- cargo test".into(),
                depth: 0,
                agent_id: Some("dev-1".into()),
            },
        ),
        env(
            2,
            TuiEvent::TaskStart {
                task_id: "review-1".into(),
                instruction: "MEDULLA_AUTOREVIEW target=task-1\nReview it".into(),
                depth: 0,
                agent_id: Some("dev-2".into()),
            },
        ),
        env(
            3,
            TuiEvent::TaskEvent {
                task_id: "review-1".into(),
                event_kind: "note".into(),
                content: "FINDINGS:\n- missing regression test".into(),
                harness: None,
            },
        ),
    ];

    let lanes = derive_agent_lanes(&events, "codex", &roster);
    let implementation = lanes
        .iter()
        .find(|lane| lane.agent_id.as_deref() == Some("dev-1"))
        .unwrap()
        .tasks
        .iter()
        .find(|task| task.task_id == "task-1")
        .unwrap();
    assert_eq!(
        implementation.review,
        Some(crate::autoreview::ReviewVerdict::Findings(vec![
            "missing regression test".into()
        ]))
    );
    let rendered = task_lines(implementation, 80);
    assert!(rendered
        .iter()
        .any(|line| line.text.contains("missing regression test")));
}

#[test]
fn task_complete_without_start_still_builds_a_lane() {
    // §3.3(4): a completion whose start was evicted must not be dropped.
    let events = vec![env(
        5,
        TuiEvent::TaskComplete {
            digest: TaskDigest {
                task_id: "orphan".into(),
                status: "done".into(),
                digest: "ok".into(),
                result_ref: None,
                usage: None,
                depth: 2,
            },
        },
    )];
    let lanes = derive_agent_lanes(&events, "OPENCODE", &[]);
    let worker = lanes.iter().find(|l| l.key == "worker:orphan").unwrap();
    assert_eq!(worker.tasks.len(), 1);
    assert_eq!(worker.tasks[0].status, TaskStatus::Done);
}

#[test]
fn session_event_folds_into_grouped_session_lane() {
    let roster = vec![AgentDescriptor {
        id: "m1".into(),
        name: "Machine".into(),
        description: String::new(),
        availability: "online".into(),
        tags: vec![],
        metadata: serde_json::Map::new(),
    }];
    let events = vec![env(
        1,
        TuiEvent::SessionEvent {
            agent_id: "m1".into(),
            session_id: "s1".into(),
            event_kind: "stdout".into(),
            content: "building".into(),
        },
    )];
    let lanes = derive_agent_lanes(&events, "TINYPLACE", &roster);
    // The machine lane comes first, its session lane grouped immediately after.
    let machine_pos = lanes.iter().position(|l| l.key == "agent:m1").unwrap();
    let session_pos = lanes
        .iter()
        .position(|l| l.session_id.as_deref() == Some("s1"))
        .unwrap();
    assert_eq!(
        session_pos,
        machine_pos + 1,
        "session groups under its machine"
    );
    let session = &lanes[session_pos];
    assert_eq!(session.turns.len(), 1);
    assert_eq!(session.turns[0].header, "stdout");
}

#[test]
fn roster_harness_metadata_tags_lane_label() {
    let mut meta = serde_json::Map::new();
    meta.insert("harness".into(), serde_json::json!("codex"));
    let roster = vec![AgentDescriptor {
        id: "dev".into(),
        name: "Dev".into(),
        description: String::new(),
        availability: "online".into(),
        tags: vec![],
        metadata: meta,
    }];
    let lanes = derive_agent_lanes(&[], "TINYPLACE", &roster);
    let dev = lanes.iter().find(|l| l.key == "agent:dev").unwrap();
    // Its own harness (CODEX) wins over the global default.
    assert_eq!(dev.label, "[CODEX] Dev");
}

#[test]
fn agent_row_helpers_lane_index_and_selectable() {
    assert_eq!(AgentRow::Separator.lane_index(), None);
    assert!(!AgentRow::Separator.selectable());
    assert_eq!(AgentRow::Lane { lane_index: 3 }.lane_index(), Some(3));
    assert!(AgentRow::Lane { lane_index: 3 }.selectable());
    assert_eq!(
        AgentRow::More {
            lane_index: 2,
            hidden: 4
        }
        .lane_index(),
        Some(2)
    );
    assert!(!AgentRow::More {
        lane_index: 2,
        hidden: 4
    }
    .selectable());
}

#[test]
fn peer_session_state_colors_and_ended_marker() {
    let events = vec![
        env(
            1,
            TuiEvent::PeerSession {
                agent_id: "m1".into(),
                session_id: "s1".into(),
                state: "idle".into(),
                harness: None,
            },
        ),
        env(
            2,
            TuiEvent::PeerSession {
                agent_id: "m1".into(),
                session_id: "s1".into(),
                state: "ended".into(),
                harness: None,
            },
        ),
    ];
    let lanes = derive_agent_lanes(&events, "TINYPLACE", &[]);
    let session = lanes
        .iter()
        .find(|l| l.session_id.as_deref() == Some("s1"))
        .unwrap();
    assert_eq!(session.turns.len(), 2);
    assert_eq!(session.turns[0].header_color.as_deref(), Some("green"));
    assert_eq!(session.turns[1].header_color.as_deref(), Some("red"));
}
