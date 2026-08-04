//! Click-through between the orchestrator's conversation and the sessions it
//! started.
//!
//! The orchestrator says "I have started three sessions" and then goes quiet
//! while they work, and until now the only way to reach one was to know which
//! agent it landed on and arrow down to it. So its conversation carries a
//! **sessions-started block** naming each one — the agent, the task, and what it
//! is doing — and selecting an entry moves the rail cursor onto that session's
//! row, which is what makes the pane show its conversation.
//!
//! The two directions are deliberately asymmetric. Going *in* is a pointer
//! gesture (the block is in the transcript, where the pointer already is) or the
//! rail's own arrows; coming *back* is `Ctrl-O`, one chord from anywhere on the
//! tab, because an operator several sessions deep should not have to find the
//! orchestrator's row again.
//!
//! Remote sessions are out of scope here: opening one arms a read-only screen
//! mirror, which is its own phase.

use super::rail::RailRow;
use super::types::App;

/// One entry of the sessions-started block.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(in crate::ui::app) struct StartedSession {
    /// The agent the session runs on — what the operator recognises it by.
    pub(in crate::ui::app) agent: String,
    /// The task that created it. Empty for a session with no task of its own.
    pub(in crate::ui::app) task_id: String,
    /// The session's current state, in the task board's vocabulary.
    pub(in crate::ui::app) status: &'static str,
    /// The rail row it selects.
    pub(in crate::ui::app) row_index: usize,
}

impl App {
    /// The sessions the orchestrator started, in rail order.
    ///
    /// Read off the rail rather than off the event stream, so the block and the
    /// tree cannot disagree about what exists — and so an entry always has a row
    /// to select. Only orchestrator-originated sessions are listed: a session a
    /// person spun up was not "started by the orchestrator", and putting it in
    /// this block would claim the orchestrator did something it did not.
    pub(in crate::ui::app) fn started_sessions(&self) -> Vec<StartedSession> {
        self.rail_rows()
            .into_iter()
            .enumerate()
            .filter_map(|(row_index, row)| {
                let RailRow::Session(session) = &row else {
                    return None;
                };
                if session.origin().is_user() {
                    return None;
                }
                let task = session.task.as_ref()?;
                Some(StartedSession {
                    agent: session.agent_id.clone().unwrap_or_default(),
                    task_id: task.task_id.clone(),
                    status: task.status.label(),
                    row_index,
                })
            })
            .collect()
    }

    /// Move focus to the session serving `task_id`, if the rail still lists it.
    ///
    /// Addressed by task rather than by row index because the rail is rebuilt
    /// every frame: a session can end between the click landing and this running,
    /// and a stale index would put the cursor on whatever took its place. A task
    /// that is no longer served is reported instead.
    pub(in crate::ui::app) fn focus_session_for_task(&mut self, task_id: &str) -> bool {
        let Some(session) = self
            .started_sessions()
            .into_iter()
            .find(|session| session.task_id == task_id)
        else {
            self.set_status(format!("No session is running {task_id}"));
            return false;
        };
        // Safe by construction: the index came from the list this call just
        // built, so nothing can have moved between resolving it and using it.
        self.tab_index = super::types::tab_pos("Agents");
        self.agent_index = session.row_index;
        self.agent_scroll = 0;
        self.chat_scroll = 0;
        // The rail owns the keyboard on a session row: there is no composer under
        // one, so leaving focus on the composer would drive a caret that is not
        // drawn.
        self.focus_agents_rail();
        self.set_status(format!(
            "{} · {} · ^O returns to the orchestrator",
            session.agent, session.task_id
        ));
        true
    }

    /// Return to the orchestrator's conversation — the `Ctrl-O` half of §A7.
    ///
    /// Puts the cursor on the orchestrator's own lane row and hands the keyboard
    /// to the composer, because that lane *is* the text box: arriving on it with
    /// focus still on the rail would mean pressing one more key before typing.
    pub(in crate::ui::app) fn focus_orchestrator(&mut self) {
        let Some(index) = self.orchestrator_row_index() else {
            self.set_status("No conversation to return to yet");
            return;
        };
        self.tab_index = super::types::tab_pos("Agents");
        self.agent_index = index;
        self.agent_scroll = 0;
        self.chat_scroll = 0;
        self.focus_agents_composer();
        self.set_status("Back to the orchestrator");
    }
}
