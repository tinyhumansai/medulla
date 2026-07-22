//! Unit tests for folding locally-observed worker activity into lanes.
//!
//! Against a real backend this is the *only* thing that can make a worker lane
//! show work: the render snapshot's events come from the backend's SSE stream,
//! whose vocabulary carries nothing about delegated tasks, so without this fold
//! a worker running a fan-out renders exactly as idle as one doing nothing.

use crate::hub::WorkerActivity;
use crate::ui::agents::merge_worker_activity;
use crate::ui::agents::{AgentLane, AgentRole, TaskStatus};

/// One observed frame.
fn record(agent: &str, task: &str, kind: &str, content: &str, at: i64) -> WorkerActivity {
    WorkerActivity {
        agent_id: agent.into(),
        task_id: task.into(),
        kind: kind.into(),
        content: content.into(),
        at,
    }
}

/// A worker lane as `derive_agent_lanes` seeds it from the roster: present, and
/// entirely empty of activity.
fn worker_lane(agent_id: &str) -> AgentLane {
    AgentLane {
        key: format!("agent:{agent_id}"),
        label: agent_id.into(),
        role: AgentRole::Worker,
        turns: Vec::new(),
        last_at: 0,
        tasks: Vec::new(),
        context_tokens: None,
        harness_label: None,
        agent_id: Some(agent_id.into()),
        session_id: None,
        parent_agent_id: None,
        descriptor: None,
        active_tasks: 0,
    }
}

#[test]
fn a_running_task_makes_its_worker_busy() {
    let mut lanes = vec![worker_lane("claude-worker")];
    merge_worker_activity(
        &mut lanes,
        &[
            record("claude-worker", "t1", "ack", "task accepted", 10),
            record("claude-worker", "t1", "status", "running Bash: ls", 20),
        ],
    );

    assert_eq!(lanes[0].active_tasks, 1, "the lane must read as busy");
    assert_eq!(lanes[0].tasks.len(), 1);
    assert_eq!(lanes[0].tasks[0].status, TaskStatus::Running);
    assert_eq!(lanes[0].last_at, 20, "last activity is the newest frame");
    // The ack is admission, not progress — counting it as a turn would make a
    // task that has done nothing look like it had.
    assert_eq!(lanes[0].tasks[0].turns, 1);
    assert_eq!(
        lanes[0].tasks[0].turn_blocks[0].content.as_deref(),
        Some("running Bash: ls")
    );
}

#[test]
fn a_reply_settles_the_task_and_frees_the_worker() {
    let mut lanes = vec![worker_lane("claude-worker")];
    merge_worker_activity(
        &mut lanes,
        &[
            record("claude-worker", "t1", "ack", "task accepted", 10),
            record("claude-worker", "t1", "status", "writing response", 20),
            record("claude-worker", "t1", "reply", "all done", 30),
        ],
    );

    assert_eq!(lanes[0].tasks[0].status, TaskStatus::Done);
    assert_eq!(
        lanes[0].active_tasks, 0,
        "a settled task must not leave the worker reading busy"
    );
}

#[test]
fn an_error_is_distinguishable_from_a_reply() {
    let mut lanes = vec![worker_lane("claude-worker")];
    merge_worker_activity(
        &mut lanes,
        &[record(
            "claude-worker",
            "t1",
            "error",
            "harness exploded",
            30,
        )],
    );
    assert_eq!(lanes[0].tasks[0].status, TaskStatus::Failed);
    assert_eq!(lanes[0].active_tasks, 0);
}

#[test]
fn concurrent_tasks_are_counted_and_kept_apart() {
    let mut lanes = vec![worker_lane("claude-worker")];
    merge_worker_activity(
        &mut lanes,
        &[
            record("claude-worker", "t1", "status", "one", 10),
            record("claude-worker", "t2", "status", "two", 11),
            record("claude-worker", "t1", "reply", "first done", 20),
        ],
    );

    assert_eq!(lanes[0].tasks.len(), 2, "one entry per task, not per frame");
    assert_eq!(lanes[0].active_tasks, 1, "only t2 is still running");
    assert_eq!(lanes[0].tasks[0].task_id, "t1", "first-seen order");
    assert_eq!(lanes[0].tasks[1].task_id, "t2");
}

#[test]
fn activity_lands_on_the_worker_that_ran_it() {
    let mut lanes = vec![worker_lane("claude-worker"), worker_lane("codex-worker")];
    merge_worker_activity(
        &mut lanes,
        &[
            record("codex-worker", "t9", "status", "running", 10),
            // A frame for a task this hub never dispatched carries no agent, and
            // must not be attributed to an arbitrary lane.
            record("", "orphan", "status", "from another harness", 11),
        ],
    );

    assert_eq!(lanes[0].active_tasks, 0, "claude-worker did nothing");
    assert_eq!(lanes[1].active_tasks, 1, "codex-worker ran it");
    assert!(lanes[0].tasks.is_empty());
}

#[test]
fn a_tier_lane_is_never_given_worker_tasks() {
    // Orchestrator/Reasoning lanes are cognitive tiers, not agents; hanging a
    // delegated task off one would claim the tier ran it.
    let mut tier = worker_lane("claude-worker");
    tier.role = AgentRole::Orchestrator;
    let mut lanes = vec![tier];
    merge_worker_activity(
        &mut lanes,
        &[record("claude-worker", "t1", "status", "running", 10)],
    );
    assert!(lanes[0].tasks.is_empty());
    assert_eq!(lanes[0].active_tasks, 0);
}
