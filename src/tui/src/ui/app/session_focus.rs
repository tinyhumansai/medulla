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
    /// The harness that agent runs, when the tree knows one.
    pub(in crate::ui::app) harness: Option<String>,
    /// The directory it works in, when the tree knows one.
    pub(in crate::ui::app) workspace: Option<String>,
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
        // The agent an entry describes is the row it sits under, which the walk
        // is already passing: sessions are grouped under their agent, so the
        // last agent row seen is this session's own. That is also where the
        // harness and the workspace come from — the session row itself carries
        // neither, and an entry that names only an id says less than the rail
        // row beside it.
        let mut agent: Option<super::rail::AgentRailRow> = None;
        let mut started = Vec::new();
        for (row_index, row) in self.rail_rows().into_iter().enumerate() {
            match row {
                RailRow::Agent(row) => agent = Some(row),
                RailRow::Session(session) => {
                    if session.origin().is_user() {
                        continue;
                    }
                    let Some(task) = session.task.as_ref() else {
                        continue;
                    };
                    // Only the agent this session is actually filed under: a
                    // session with no agent sits outside every group, so the row
                    // above it describes somebody else.
                    let owner = agent.as_ref().filter(|owner| {
                        Some(owner.agent_id.as_str()) == session.agent_id.as_deref()
                    });
                    started.push(StartedSession {
                        agent: owner
                            .map(|owner| owner.label())
                            .or_else(|| session.agent_id.clone())
                            .unwrap_or_default(),
                        harness: owner.and_then(|owner| owner.harness().map(str::to_string)),
                        workspace: owner.and_then(|owner| owner.workspace().map(str::to_string)),
                        task_id: task.task_id.clone(),
                        status: task.status.label(),
                        row_index,
                    });
                }
                _ => {}
            }
        }
        started
    }

    /// Move focus to the session serving `task_id`, if the rail still lists it.
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
        self.tab_index = super::types::tab_pos("Agents");
        self.set_rail_cursor_in(&rows, &lanes, row_index);
        self.agent_scroll = 0;
        self.chat_scroll = 0;
        // The rail owns the keyboard on a session row: there is no composer under
        // one, so leaving focus on the composer would drive a caret that is not
        // drawn.
        self.focus_agents_rail();
        self.set_status(format!("{} · {}", agent, session_task_id));
        true
    }
}
