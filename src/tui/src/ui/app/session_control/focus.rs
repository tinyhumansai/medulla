//! Bringing the operator to the session running a given task.
//!
//! This is what a click on a task — from anywhere that names one — resolves to.
//! It used to be half of a two-way trip between the orchestrator's conversation
//! and the sessions it started, with `Ctrl-O` as the way back; the conversation
//! is gone from the TUI, so what is left is the direction that still means
//! something: given a task id, put the operator in front of the session serving
//! it.

use super::super::rail::RailRow;
use super::super::types::App;

impl App {
    /// Move the rail cursor onto the session serving `task_id`.
    ///
    /// Returns whether such a session was found, so a caller can report the miss
    /// in its own words.
    ///
    /// Addressed by task rather than by row index because the rail is rebuilt
    /// every frame: a session can end between the click landing and this running,
    /// and a stale index would put the cursor on whatever took its place. A task
    /// that is no longer served is reported instead.
    pub(in crate::ui::app) fn focus_session_for_task(&mut self, task_id: &str) -> bool {
        // Keep the rows that yielded the offset through the cursor write. The
        // local PTY registry can change while this event is handled; rebuilding
        // here would let an insertion above the target retarget the cursor.
        let lanes = self.lanes();
        let rows = self.rail_rows_in(&lanes);
        let Some((row_index, agent, session_task_id)) =
            rows.iter().enumerate().find_map(|(index, row)| {
                let RailRow::Session(session) = row else {
                    return None;
                };
                let task = session.task.as_ref()?;
                (task.task_id == task_id && !session.origin().is_user()).then(|| {
                    (
                        index,
                        session.agent_id.clone().unwrap_or_default(),
                        task.task_id.clone(),
                    )
                })
            })
        else {
            self.set_status(format!("No session is running {task_id}"));
            return false;
        };
        self.tab_index = super::super::types::tab_pos("Sessions");
        self.set_rail_cursor_in(&rows, &lanes, row_index);
        self.agent_scroll = 0;
        self.set_status(format!("{agent} · {session_task_id}"));
        true
    }
}
