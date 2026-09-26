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
/// before the first function lane, and per-task sublanes (running first, capped
/// at `max_subtasks`).
///
/// The fixed-cap form of [`agent_row_model_paged`], for callers that have no
/// per-lane expansion state to consult.
pub fn agent_row_model(lanes: &[AgentLane], max_subtasks: usize) -> Vec<AgentRow> {
    agent_row_model_paged(lanes, max_subtasks, |_| 0)
}

/// Build the ordered Agents-list rows, revealing a lane's sublanes a page at a
/// time.
///
/// A lane shows `page` sublanes plus `page` more for each extra page
/// `extra_pages` reports for it, so an operator paging through a busy agent's
/// tasks walks `10`, `20`, `30` rather than being handed all of them at once.
/// `extra_pages` is keyed off the lane itself (its
/// [`key`](AgentLane::key) is the stable identity across folds) because
/// `lane_index` shifts as lanes come and go.
///
/// The trailing [`AgentRow::More`] carries both directions: it counts what is
/// still hidden while anything is, and once the lane is fully revealed it stays
/// on as the row that folds it back to one page. A lane that fits inside one
/// page never gets one.
pub fn agent_row_model_paged(
    lanes: &[AgentLane],
    page: usize,
    extra_pages: impl Fn(&AgentLane) -> usize,
) -> Vec<AgentRow> {
    let mut rows = Vec::new();
    let first_fn = lanes.iter().position(|l| l.role.is_function());
    for (lane_index, lane) in lanes.iter().enumerate() {
        if Some(lane_index) == first_fn {
            rows.push(AgentRow::Separator);
        }
        rows.push(AgentRow::Lane { lane_index });
        if lane.role == AgentRole::Agent && lane.key.starts_with("agent:") && !lane.tasks.is_empty()
        {
            let ordered = ordered_tasks(&lane.tasks);
            let cap = page.saturating_mul(extra_pages(lane).saturating_add(1));
            let shown = ordered.len().min(cap);
            let hidden = ordered.len() - shown;
            // The overflow row closes the lane whenever it is drawn — including
            // the fully-revealed case, where it is the `show less` control. The
            // last sublane only takes the closing branch when nothing follows
            // it, or the lane ends on two `└` in a row.
            let overflow = hidden > 0 || shown > page;
            for (i, task) in ordered.iter().take(shown).enumerate() {
                rows.push(AgentRow::Sub {
                    lane_index,
                    task: task.clone(),
                    last: !overflow && i == shown - 1,
                });
            }
            if overflow {
                rows.push(AgentRow::More { lane_index, hidden });
            }
        }
    }
    rows
}
