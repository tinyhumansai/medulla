//! The small navigation helpers the keyboard and the pointer share: rail
//! movement, prompt-history recall, thread creation, the mouse-capture toggle,
//! and the live-screen subscription that follows the rail cursor.
//!
//! They live apart from either input surface because both drive them — a lane is
//! reached by `Alt`+`↑` and by clicking it, and neither path should own the
//! other's helper.

use super::super::rail::RailRow;
use super::super::types::{App, Cmd};
#[cfg(test)]
use crate::ui::agents::{agent_row_model_paged, AgentRow};
use crate::ui::agents::{AgentRole, TaskStatus};

/// How many of a lane's task sublanes one page reveals.
///
/// A busy agent accumulates far more tasks than the rail can show beside the
/// transcript, so the lane shows a page and offers the rest behind its `+N more`
/// row. Ten fills the space an unexpanded lane can spare without pushing the
/// lanes under it off screen.
pub(in crate::ui::app) const SUBTASK_PAGE: usize = 10;

impl App {
    /// The current Sessions rail rows, each lane paged to whatever the operator
    /// has expanded it to.
    #[cfg(test)]
    pub(in crate::ui::app) fn agent_rows(&self) -> Vec<AgentRow> {
        let lanes = self.lanes();
        agent_row_model_paged(&lanes, SUBTASK_PAGE, |lane| {
            self.subtask_pages.get(&lane.key).copied().unwrap_or(0)
        })
    }

    /// Page the lane under the cursor open by one page, or fold a fully-revealed
    /// lane back to its first page.
    ///
    /// Returns whether the cursor was on an overflow row at all, so the callers
    /// that share this — `Enter` from the keyboard and a click from the pointer —
    /// can fall through to their ordinary behaviour when it was not.
    ///
    /// The cursor is moved to wherever the overflow row ended up, which is what
    /// makes repeated `Enter` walk down through the pages: the row it acts on
    /// slides down as sublanes appear above it, and the viewport follows the
    /// cursor, so the rows just revealed are the ones on screen.
    pub(in crate::ui::app) fn page_subtasks(&mut self) -> bool {
        let lanes = self.lanes();
        let rows = self.rail_rows_in(&lanes);
        let cursor = self.rail_cursor_in(&rows, &lanes);
        let Some(RailRow::Overflow { lane_index, hidden }) = rows.get(cursor) else {
            return false;
        };
        let (lane_index, hidden) = (*lane_index, *hidden);
        let Some(lane) = lanes.get(lane_index) else {
            return false;
        };
        let key = lane.key.clone();
        let total = lane.tasks.len();
        if hidden > 0 {
            *self.subtask_pages.entry(key.clone()).or_insert(0) += 1;
            let revealed = self.revealed_subtasks(&key).min(total);
            // What the row will say once it is redrawn — the page that reveals
            // the last task turns it into the collapse control, and a status
            // still offering "more" would contradict the row under the cursor.
            let next = if revealed < total {
                "↵ for more"
            } else {
                "↵ collapses"
            };
            self.set_status(format!("Showing {revealed} of {total} tasks · {next}"));
        } else {
            self.subtask_pages.remove(&key);
            self.set_status(format!(
                "Showing {} of {total} tasks · ↵ expands",
                SUBTASK_PAGE.min(total)
            ));
        }
        self.follow_overflow_row(&key);
        true
    }

    /// How many sublanes a lane reveals at its current expansion.
    pub(in crate::ui::app) fn revealed_subtasks(&self, key: &str) -> usize {
        SUBTASK_PAGE.saturating_mul(
            self.subtask_pages
                .get(key)
                .copied()
                .unwrap_or(0)
                .saturating_add(1),
        )
    }

    /// Put the cursor back on a lane's overflow row after its rows have moved.
    ///
    /// Falls back to the lane's first session, and then to clamping, so a lane
    /// that no longer has an overflow row cannot strand the cursor past the end
    /// of the rail. The lane header it used to fall back to is gone with the
    /// agent tier, and the first session of that lane is the nearest row left.
    fn follow_overflow_row(&mut self, lane_key: &str) {
        let lanes = self.lanes();
        let rows = self.rail_rows_in(&lanes);
        let lane_index = lanes.iter().position(|lane| lane.key == lane_key);
        let found = rows
            .iter()
            .position(|row| {
                matches!(row, RailRow::Overflow { lane_index: l, .. } if Some(*l) == lane_index)
            })
            .or_else(|| {
                rows.iter()
                    .position(|row| row.lane_index().is_some() && row.lane_index() == lane_index)
            });
        self.set_rail_cursor_in(
            &rows,
            &lanes,
            found.unwrap_or_else(|| self.rail_index.min(rows.len().saturating_sub(1))),
        );
    }

    /// The number of body rows a list pane can show for the current terminal
    /// height.
    pub(in crate::ui::app) fn visible_count(&self) -> usize {
        (self.area.height as usize).saturating_sub(13).max(5)
    }

    /// Move the Sessions-rail cursor to the next/previous selectable row.
    ///
    /// Not every row can hold the cursor: the `── functions ──` separator is a
    /// label, not a destination. So this steps over it to the next lane or task
    /// rather than stopping on it, and a cursor that would leave the list stays
    /// where it was. The `+N more` row *is* a destination — it is the control
    /// that pages its lane open.
    pub(in crate::ui::app) fn move_rail_index(&mut self, up: bool) {
        let lanes = self.lanes();
        let rows = self.rail_rows_in(&lanes);
        if rows.is_empty() {
            return;
        }
        // `rail_index` is only the last rendered offset. Resolve the anchor
        // first: a local session may have appeared above it since that frame,
        // and stepping from the old offset would select the wrong neighbour.
        let clamped = self.rail_cursor_in(&rows, &lanes);
        let step: i64 = if up { -1 } else { 1 };
        let mut next = clamped as i64 + step;
        while next >= 0 && (next as usize) < rows.len() && !rows[next as usize].selectable() {
            next += step;
        }
        let next = if next < 0 || next as usize >= rows.len() {
            clamped
        } else {
            next as usize
        };
        self.set_rail_cursor_in(&rows, &lanes, next);
        #[cfg(feature = "workflows")]
        self.sync_selected_workflow_run();
    }

    /// Open a new thread and focus the conversation.
    ///
    /// A thread is opened, not reset: several conversations can be in flight at
    /// once, and clearing the one you were in would throw away the transcript
    /// you were keeping. Nothing is inherited from the current thread — that is
    /// the whole difference from the fork this replaced.
    pub(in crate::ui::app) fn new_thread(&mut self) {
        self.runtime.new_session();
        self.draft = crate::ui::composer::Draft::new();
        self.agent_scroll = 0;
        self.reset_rail_cursor();
        self.tab_index = super::super::types::tab_pos("Sessions");
        self.refresh_snapshot();
        let name = self
            .snapshot
            .threads
            .get(self.active_thread_idx())
            .map(|t| t.name.clone())
            .unwrap_or_else(|| "main".into());
        self.set_status(format!("Opened {name} · ^↑↓ switches threads"));
    }

    /// Toggle mouse capture and note the new mode in the status line.
    pub(in crate::ui::app) fn toggle_mouse(&mut self) {
        self.mouse_capture = !self.mouse_capture;
        self.set_status(if self.mouse_capture {
            "Mouse captured — click tabs/pages/lanes, wheel scrolls, drag to select and copy"
        } else {
            "Mouse released — native click-drag selection & copy restored"
        });
    }

    /// Point the live screen subscription at whatever is selected now.
    ///
    /// Returns a command only when the target changed, so this is safe to call
    /// after every selection move: every subscribe carries `resync: true`, and
    /// re-issuing one per keystroke would make the worker resend a full frame
    /// each time — the expensive thing the protocol exists to avoid.
    ///
    /// Only a selected *task* subscribes. A worker lane names no task, and
    /// streams are addressed by task: there is nothing to ask for, and guessing
    /// one of a worker's tasks would watch work the operator did not point at.
    pub(in crate::ui::app) fn retarget_watch(&mut self) -> Option<Cmd> {
        let desired = self.watch_target();
        if desired == self.watching {
            return None;
        }
        let stop = self.watching.take();
        self.watching = desired.clone();
        Some(Cmd::WatchTask {
            stop,
            start: desired,
        })
    }

    /// The `(worker address, task id)` the current selection asks to watch.
    pub(in crate::ui::app) fn watch_target(&self) -> Option<(String, String)> {
        let lanes = self.lanes();
        let rows = self.rail_rows_in(&lanes);
        self.watch_target_in(&rows, &lanes)
    }

    /// Resolve a watch target from one coherent rail and lane snapshot.
    fn watch_target_in(
        &self,
        rows: &[RailRow],
        lanes: &[crate::ui::agents::AgentLane],
    ) -> Option<(String, String)> {
        // Only on the Sessions tab: leaving it releases the subscription.
        if self.tab() != "Sessions" {
            return None;
        }
        let row = rows.get(self.rail_cursor_in(rows, lanes))?;
        let RailRow::Session(session) = row else {
            return None;
        };
        let task = session.task.as_ref()?;
        let lane = lanes.get(session.lane_index?)?;
        // `Agent` is main's name for a roster agent / delegated task / peer
        // session — the tiers above it (orchestrator, reasoning, compress) run
        // no watchable harness.
        if lane.role != AgentRole::Agent {
            return None;
        }
        let agent_id = lane.agent_id.as_deref()?;
        // Streams are addressed to the worker's link address; a lane
        // carries its roster id. A worker added by address is registered under
        // it, so that is the fallback.
        let address = self
            .runtime
            .workers()
            .into_iter()
            .find(|w| w.id == agent_id)
            .map(|w| w.address)
            .unwrap_or_else(|| agent_id.to_string());
        Some((address, task.task_id.clone()))
    }

    /// The selected running task eligible for destructive termination.
    pub(in crate::ui::app) fn kill_target(&self) -> Option<(String, String)> {
        let lanes = self.lanes();
        let rows = self.rail_rows_in(&lanes);
        let row = rows.get(self.rail_cursor_in(&rows, &lanes))?;
        let task = row.task()?;
        (task.status == TaskStatus::Running)
            .then(|| self.watch_target_in(&rows, &lanes))
            .flatten()
    }
}
