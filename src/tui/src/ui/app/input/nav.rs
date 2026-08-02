//! The small navigation helpers the keyboard and the pointer share: rail
//! movement, prompt-history recall, thread creation, the mouse-capture toggle,
//! and the live-screen subscription that follows the rail cursor.
//!
//! They live apart from either input surface because both drive them — a lane is
//! reached by `Alt`+`↑` and by clicking it, and neither path should own the
//! other's helper.

use super::super::rail::RailRow;
use super::super::types::{App, Cmd};
use crate::ui::agents::{agent_row_model, AgentRole, AgentRow, TaskStatus};
use crate::ui::composer::Draft;

impl App {
    /// The current Agents-list rows (lanes flattened with a hidden-row cap).
    pub(in crate::ui::app) fn agent_rows(&self) -> Vec<AgentRow> {
        agent_row_model(&self.lanes(), 8)
    }

    /// The number of body rows a list pane can show for the current terminal
    /// height.
    pub(in crate::ui::app) fn visible_count(&self) -> usize {
        (self.area.height as usize).saturating_sub(13).max(5)
    }

    /// Move the Agents-rail cursor to the next/previous selectable row.
    ///
    /// Not every row can hold the cursor: the `── functions ──` separator and
    /// the `+N more` counter are labels, not destinations. So this steps over
    /// them to the next lane or task rather than stopping on one, and a cursor
    /// that would leave the list stays where it was.
    pub(in crate::ui::app) fn move_agent_index(&mut self, up: bool) {
        let rows = self.rail_rows();
        if rows.is_empty() {
            return;
        }
        let clamped = self.agent_index.min(rows.len() - 1);
        let step: i64 = if up { -1 } else { 1 };
        let mut next = clamped as i64 + step;
        while next >= 0 && (next as usize) < rows.len() && !rows[next as usize].selectable() {
            next += step;
        }
        self.agent_index = if next < 0 || next as usize >= rows.len() {
            clamped
        } else {
            next as usize
        };
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
        self.chat_scroll = 0;
        self.agent_scroll = 0;
        self.agent_index = 0;
        self.tab_index = super::super::types::tab_pos("Agents");
        self.refresh_snapshot();
        let name = self
            .snapshot
            .threads
            .get(self.active_thread_idx())
            .map(|t| t.name.clone())
            .unwrap_or_else(|| "main".into());
        self.set_status(format!("Opened {name} · ^↑↓ switches threads"));
    }

    /// Recall an older prompt from history into the composer.
    pub(in crate::ui::app) fn recall_older(&mut self) {
        let next = (self.history.len() as i64 - 1).min(self.history_index + 1);
        if next >= 0 {
            self.history_index = next;
            let recalled = self
                .history
                .get(self.history.len() - 1 - next as usize)
                .cloned()
                .unwrap_or_default();
            self.draft = Draft {
                cursor: recalled.chars().count(),
                text: recalled,
            };
        }
    }

    /// Recall a newer prompt from history (or clear back to an empty draft).
    pub(in crate::ui::app) fn recall_newer(&mut self) {
        if self.history_index >= 0 {
            let next = self.history_index - 1;
            self.history_index = next;
            let recalled = if next >= 0 {
                self.history
                    .get(self.history.len() - 1 - next as usize)
                    .cloned()
                    .unwrap_or_default()
            } else {
                String::new()
            };
            self.draft = Draft {
                cursor: recalled.chars().count(),
                text: recalled,
            };
        }
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
        // Only on the Agents tab: leaving it releases the subscription.
        if self.tab() != "Agents" {
            return None;
        }
        let rows = self.rail_rows();
        let row = rows.get(self.agent_index.min(rows.len().saturating_sub(1)))?;
        let RailRow::Agent(AgentRow::Sub {
            task, lane_index, ..
        }) = row
        else {
            return None;
        };
        let lanes = self.lanes();
        let lane = lanes.get(*lane_index)?;
        // `Agent` is main's name for a roster agent / delegated task / peer
        // session — the tiers above it (orchestrator, reasoning, compress) run
        // no watchable harness.
        if lane.role != AgentRole::Agent {
            return None;
        }
        let agent_id = lane.agent_id.as_deref()?;
        // Streams are addressed to the worker's tiny.place address; a lane
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
        let rows = self.rail_rows();
        let row = rows.get(self.agent_index.min(rows.len().saturating_sub(1)))?;
        let RailRow::Agent(AgentRow::Sub { task, .. }) = row else {
            return None;
        };
        (task.status == TaskStatus::Running)
            .then(|| self.watch_target())
            .flatten()
    }
}
