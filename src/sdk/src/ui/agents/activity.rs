//! Fold locally-observed worker activity into the Agents-view lanes.
//!
//! [`derive_agent_lanes`](super::derive_agent_lanes) fills a lane's tasks and
//! turns from the render snapshot's event log — which is filled from the
//! *backend's* SSE stream. That stream carries `user`, `assistant`, `cycle_*`
//! and the token deltas, and nothing at all about delegated tasks. So against a
//! real backend a worker lane is structurally guaranteed to render idle, no
//! matter how much work is running: the only events that could say otherwise are
//! produced solely by the scripted mock runtime.
//!
//! The orchestrator hub has the truth in-process — it dispatched the task and
//! every frame comes back through its inbox. This module projects that onto the
//! lanes it belongs to, the same way [`merge_worker_roster`](super::roster) puts
//! locally-registered workers on the tab in the first place.

use std::collections::HashMap;

use crate::hub::WorkerActivity;

use super::types::{AgentLane, AgentRole, TaskState, TaskStatus, TurnBlock};

/// Fold `activity` into the worker lanes of `lanes`, in place.
///
/// Only lanes that already exist are filled: a lane is a roster entry, and
/// inventing one from a stray task would put a worker on screen that this hub
/// does not actually have.
pub fn merge_worker_activity(lanes: &mut [AgentLane], activity: &[WorkerActivity]) {
    if activity.is_empty() {
        return;
    }
    let mut by_agent: HashMap<&str, Vec<&WorkerActivity>> = HashMap::new();
    for record in activity {
        by_agent
            .entry(record.agent_id.as_str())
            .or_default()
            .push(record);
    }

    for lane in lanes.iter_mut() {
        if lane.role != AgentRole::Worker {
            continue;
        }
        let Some(agent_id) = lane.agent_id.clone() else {
            continue;
        };
        let Some(records) = by_agent.get(agent_id.as_str()) else {
            continue;
        };
        let tasks = fold_tasks(records);
        lane.active_tasks = tasks
            .iter()
            .filter(|t| t.status == TaskStatus::Running)
            .count() as i64;
        lane.last_at = tasks
            .iter()
            .map(|t| t.last_at)
            .max()
            .unwrap_or(lane.last_at);
        lane.tasks = tasks;
    }
}

/// Group one worker's records into per-task state, in first-seen order.
fn fold_tasks(records: &[&WorkerActivity]) -> Vec<TaskState> {
    let mut order: Vec<String> = Vec::new();
    let mut by_task: HashMap<String, TaskState> = HashMap::new();

    for record in records {
        let state = by_task.entry(record.task_id.clone()).or_insert_with(|| {
            order.push(record.task_id.clone());
            TaskState {
                task_id: record.task_id.clone(),
                status: TaskStatus::Running,
                turns: 0,
                last_at: record.at,
                turn_blocks: Vec::new(),
                attention: None,
                question_id: None,
            }
        });
        state.last_at = record.at;
        match record.kind.as_str() {
            // An ack only says the worker admitted it; it is not progress and
            // showing it as a turn would make an idle task look busy.
            "ack" => {}
            "reply" => {
                state.status = TaskStatus::Done;
                state.turns += 1;
                state.turn_blocks.push(block(record));
            }
            "error" => {
                state.status = TaskStatus::Failed;
                state.turns += 1;
                state.turn_blocks.push(block(record));
            }
            _ => {
                state.turns += 1;
                state.turn_blocks.push(block(record));
            }
        }
    }
    order
        .into_iter()
        .filter_map(|id| by_task.remove(&id))
        .collect()
}

/// One activity record as a displayable turn.
fn block(record: &WorkerActivity) -> TurnBlock {
    TurnBlock {
        at: record.at,
        header: record.kind.clone(),
        header_color: None,
        reasoning: None,
        content: Some(record.content.clone()),
        tools: Vec::new(),
    }
}
