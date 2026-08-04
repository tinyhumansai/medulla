//! Tests for the event fold and the Agents-list row model.

use super::env;
use crate::runtime::AgentDescriptor;
use crate::ui::agents::*;
use crate::ui::events::{TaskDigest, TuiEvent, Usage};

#[test]
fn the_orchestrator_is_the_only_tier_lane() {
    let lanes = derive_agent_lanes(&[], "OPENCODE", &[]);
    // The manager tier and the compress function are hidden: the backend
    // streams the orchestrator and the agents it manages, nothing between.
    assert_eq!(lanes.len(), 1);
    assert_eq!(lanes[0].label, "orchestrator");
    assert_eq!(lanes[0].role, AgentRole::Orchestrator);
}

#[test]
fn inference_end_folds_into_the_orchestrator_tier() {
    let events = vec![env(
        1,
        TuiEvent::InferenceEnd {
            tier: "orchestrator".into(),
            op: "execute_step".into(),
            model: Some("gpt".into()),
            duration_ms: 42,
            usage: Some(Usage {
                input_tokens: 100,
                output_tokens: 20,
                ..Default::default()
            }),
            content: Some("hi".into()),
            reasoning: None,
            tool_calls: None,
        },
    )];
    let lanes = derive_agent_lanes(&events, "", &[]);
    let orchestrator = &lanes[0];
    assert_eq!(orchestrator.turns.len(), 1);
    assert!(orchestrator.turns[0]
        .header
        .contains("execute_step · gpt · 42ms"));
    assert_eq!(orchestrator.context_tokens, Some(100));
}

#[test]
fn a_manager_turn_is_dropped_rather_than_given_a_lane() {
    let events = vec![env(
        1,
        TuiEvent::InferenceEnd {
            tier: "reasoning".into(),
            op: "execute_step".into(),
            model: None,
            duration_ms: 5,
            usage: None,
            content: Some("planning".into()),
            reasoning: None,
            tool_calls: None,
        },
    )];
    let lanes = derive_agent_lanes(&events, "", &[]);
    assert_eq!(lanes.len(), 1);
    assert!(lanes[0].turns.is_empty(), "the manager tier has no lane");
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
                contract: None,
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
                        ..Default::default()
                    }),
                    depth: 2,
                    contract: None,
                    evidence: None,
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
        workspace_id: None,
        host_id: None,
        template_id: None,
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
                contract: None,
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
    // No function lane is produced any more, so no divider is either.
    assert!(!rows.iter().any(|r| matches!(r, AgentRow::Separator)));
}

/// Lanes for one rostered agent carrying `tasks` started tasks.
fn lanes_with_tasks(tasks: usize) -> Vec<AgentLane> {
    let roster = vec![AgentDescriptor {
        id: "dev".into(),
        name: "Dev".into(),
        description: String::new(),
        availability: "online".into(),
        workspace_id: None,
        host_id: None,
        template_id: None,
        tags: vec![],
        metadata: serde_json::Map::new(),
    }];
    let events = (0..tasks)
        .map(|i| {
            env(
                i as u64,
                TuiEvent::TaskStart {
                    task_id: format!("t{i}"),
                    instruction: "x".into(),
                    depth: 2,
                    agent_id: Some("dev".into()),
                    contract: None,
                },
            )
        })
        .collect::<Vec<_>>();
    derive_agent_lanes(&events, "TINYPLACE", &roster)
}

/// The sublane count and the overflow row's hidden count, for one expansion.
fn paged_counts(lanes: &[AgentLane], page: usize, extra: usize) -> (usize, Option<usize>) {
    let rows = agent_row_model_paged(lanes, page, |_| extra);
    let subs = rows
        .iter()
        .filter(|r| matches!(r, AgentRow::Sub { .. }))
        .count();
    let hidden = rows.iter().find_map(|r| match r {
        AgentRow::More { hidden, .. } => Some(*hidden),
        _ => None,
    });
    (subs, hidden)
}

#[test]
fn each_extra_page_reveals_another_page_of_sublanes() {
    let lanes = lanes_with_tasks(25);
    assert_eq!(paged_counts(&lanes, 10, 0), (10, Some(15)));
    assert_eq!(paged_counts(&lanes, 10, 1), (20, Some(5)));
}

#[test]
fn a_fully_revealed_lane_keeps_an_overflow_row_to_collapse_with() {
    let lanes = lanes_with_tasks(25);
    // Every task is on screen, but the row stays on — it is the way back to one
    // page, and `hidden: 0` is what the renderer reads as "show less".
    assert_eq!(paged_counts(&lanes, 10, 2), (25, Some(0)));
    // Expanding past the end reveals no more than the lane holds.
    assert_eq!(paged_counts(&lanes, 10, 9), (25, Some(0)));
}

#[test]
fn a_lane_within_one_page_has_no_overflow_row() {
    let lanes = lanes_with_tasks(6);
    assert_eq!(paged_counts(&lanes, 10, 0), (6, None));
    // Nothing is hidden and nothing was expanded, so there is nothing to offer.
    let rows = agent_row_model_paged(&lanes, 10, |_| 0);
    assert!(matches!(
        rows.iter()
            .rev()
            .find(|r| matches!(r, AgentRow::Sub { .. })),
        Some(AgentRow::Sub { last: true, .. })
    ));
}

#[test]
fn the_overflow_row_closes_the_lane_rather_than_the_last_sublane() {
    let lanes = lanes_with_tasks(25);
    // Whether it counts hidden rows or offers to collapse, the overflow row is
    // the lane's last line — so no sublane above it may also draw the closing
    // branch, or the lane ends on two `└`.
    for extra in [0, 1, 2] {
        let rows = agent_row_model_paged(&lanes, 10, |_| extra);
        assert!(
            !rows
                .iter()
                .any(|r| matches!(r, AgentRow::Sub { last: true, .. })),
            "a sublane closed the lane under the overflow row at extra={extra}"
        );
    }
}

#[test]
fn the_fixed_cap_model_matches_a_single_unexpanded_page() {
    let lanes = lanes_with_tasks(25);
    let fixed = agent_row_model(&lanes, 10);
    let paged = agent_row_model_paged(&lanes, 10, |_| 0);
    assert_eq!(fixed.len(), paged.len());
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
                contract: None,
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
                contract: None,
                evidence: None,
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
                contract: None,
                evidence: None,
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
        workspace_id: None,
        host_id: None,
        template_id: None,
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
        workspace_id: None,
        host_id: None,
        template_id: None,
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
    // The overflow row pages its lane open, so the cursor must reach it.
    assert!(AgentRow::More {
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
