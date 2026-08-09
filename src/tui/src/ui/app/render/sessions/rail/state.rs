//! What a session row on the Sessions rail is waiting for.
//!
//! This module used to carry the whole vocabulary of a *lane* row — its
//! presence glyph, its state suffix, its style — because a lane had a row of its
//! own on the rail. The rail lists sessions now, so what is left is the one
//! question a session row still asks: is the harness behind this task stopped on
//! something only a person can answer, and what is it?

use std::collections::HashSet;

use crate::worker::pty::HarnessAttention;

use super::super::super::super::types::App;

impl App {
    /// The cue a task's backing harness is raising, if any.
    ///
    /// Returns the whole cue rather than a boolean so the row can tell a harness
    /// that *died* from one that merely wants input — the first is bad news and
    /// draws red, the second pulses the attention colour.
    ///
    /// The attached pane is excluded: its prompt is already on screen in front
    /// of the person the signal is for, so flagging it would ask them to go and
    /// look at what they are looking at.
    pub(super) fn task_attention_cue(
        &self,
        task_id: &str,
        waiting_sessions: &HashSet<String>,
        now: i64,
    ) -> Option<HarnessAttention> {
        let harnesses = self.local_sessions.as_ref()?;
        let session_id = task_waiting_session(
            task_id,
            self.harness_focus.attached_to(),
            waiting_sessions,
            |task_id| harnesses.session_for_task(task_id),
        )?;
        // The same `row_cue` the waiting-set was built from, so the row and the
        // "N waiting on you" counter can never disagree about what this task is
        // waiting for.
        let row = harnesses.sessions.row(&session_id)?;
        crate::worker::pty::row_cue(&row, now)
    }
}

/// Return a task's waiting session unless its pane is already attached.
pub(super) fn task_waiting_session(
    task_id: &str,
    attached: Option<&str>,
    waiting_sessions: &HashSet<String>,
    mut resolve: impl FnMut(&str) -> Option<String>,
) -> Option<String> {
    resolve(task_id)
        .filter(|session| attached != Some(session.as_str()) && waiting_sessions.contains(session))
}
