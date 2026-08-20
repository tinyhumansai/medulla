//! Paging and rendering of sessions nested below one agent rail row.
//!
//! This keeps selection and retention rules together: a page can retain the
//! cursor's task or one with a live workflow run, and its overflow count must
//! describe the tasks actually omitted after those retention rules apply.

use medulla::control_socket::HarnessRunRegistry;

use super::{run_rows_under, AgentGroup, RailAnchor, RailRow};
use crate::ui::agents::AgentLane;

/// Push one agent's selected session rows and its optional paging control.
pub(super) fn push_group(
    rows: &mut Vec<RailRow>,
    group: &mut AgentGroup,
    runs: &HarnessRunRegistry,
    lanes: &[AgentLane],
    anchor: Option<&RailAnchor>,
) {
    let task_limit = group.visible_tasks;
    let mut visible_tasks = 0;
    let mut hidden_tasks = 0;
    let pinned_task = group
        .row
        .lane_index
        .and_then(|index| lanes.get(index))
        .and_then(|lane| match anchor {
            Some(RailAnchor::Task {
                lane: anchored_lane,
                task_id,
            }) if anchored_lane == &lane.key => Some(task_id.as_str()),
            _ => None,
        });
    let mut shown_sessions: Vec<_> = group
        .sessions
        .iter_mut()
        .filter(|session| {
            if session.task.is_none() {
                return true;
            }
            visible_tasks += 1;
            let shown = visible_tasks <= task_limit
                || session
                    .task
                    .as_ref()
                    .is_some_and(|task| Some(task.task_id.as_str()) == pinned_task)
                || !run_rows_under(session, runs).is_empty();
            if !shown {
                hidden_tasks += 1;
            }
            shown
        })
        .collect();
    // A pinned task or one with an active workflow run may extend the fold's
    // nominal page. Recount what was actually omitted so the overflow action
    // neither overstates the remainder nor survives after showing every task.
    let show_overflow = group.overflow && (group.hidden == 0 || hidden_tasks > 0);
    let shown = shown_sessions.len();
    for (index, session) in shown_sessions.iter_mut().enumerate() {
        session.last = !show_overflow && index + 1 == shown;
        let session = Box::new(session.clone());
        let run_rows = run_rows_under(&session, runs);
        rows.push(RailRow::Session(session));
        rows.extend(run_rows);
    }
    if show_overflow {
        rows.push(RailRow::Overflow {
            lane_index: group.row.lane_index.unwrap_or(0),
            hidden: hidden_tasks,
        });
    }
}

#[cfg(test)]
mod tests;
