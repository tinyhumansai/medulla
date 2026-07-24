//! Agents-list row derivation: ordering a lane's tasks and flattening the lanes
//! into the printable [`AgentRow`] sequence (lane headers, the functions divider,
//! and capped per-task sublanes).

use super::types::{AgentLane, AgentRole, AgentRow, TaskState, TaskStatus};

/// Running tasks first, then most-recently-active.
pub fn ordered_tasks(tasks: &[TaskState]) -> Vec<TaskState> {
    let mut v = tasks.to_vec();
    v.sort_by(|a, b| {
        let rank = |t: &TaskState| {
            if t.status == TaskStatus::Running {
                0
            } else {
                1
            }
        };
        rank(a).cmp(&rank(b)).then(b.last_at.cmp(&a.last_at))
    });
    v
}

/// Build the ordered Agents-list rows: each lane, the `── functions ──` divider
/// before the first function lane, and per-task sublanes (running first, capped).
pub fn agent_row_model(lanes: &[AgentLane], max_subtasks: usize) -> Vec<AgentRow> {
    let mut rows = Vec::new();
    let first_fn = lanes.iter().position(|l| l.role.is_function());
    for (lane_index, lane) in lanes.iter().enumerate() {
        if Some(lane_index) == first_fn {
            rows.push(AgentRow::Separator);
        }
        rows.push(AgentRow::Lane { lane_index });
        if lane.role == AgentRole::Worker
            && lane.key.starts_with("agent:")
            && !lane.tasks.is_empty()
        {
            let ordered = ordered_tasks(&lane.tasks);
            let shown = ordered.len().min(max_subtasks);
            let hidden = ordered.len() - shown;
            for (i, task) in ordered.iter().take(shown).enumerate() {
                rows.push(AgentRow::Sub {
                    lane_index,
                    task: Box::new(task.clone()),
                    last: hidden == 0 && i == shown - 1,
                });
            }
            if hidden > 0 {
                rows.push(AgentRow::More { lane_index, hidden });
            }
        }
    }
    rows
}
