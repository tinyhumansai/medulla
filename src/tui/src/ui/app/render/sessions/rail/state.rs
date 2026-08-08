//! Whether a session row on the Sessions rail is waiting on the operator.
//!
//! This module used to carry the whole vocabulary of a *lane* row — its
//! presence glyph, its state suffix, its style — because a lane had a row of its
//! own on the rail. The rail lists sessions now, so what is left is the one
//! question a session row still asks: is the harness behind this task stopped on
//! something only a person can answer.

use std::collections::HashSet;

use super::super::super::super::types::App;

impl App {
    /// Whether the task row is backed by a local harness waiting on input.
    ///
    /// The attached pane is excluded: its prompt is already on screen in front
    /// of the person the signal is for, so flagging it would ask them to go and
    /// look at what they are looking at.
    pub(super) fn task_attention(&self, task_id: &str, waiting_sessions: &HashSet<String>) -> bool {
        let Some(harnesses) = self.local_sessions.as_ref() else {
            return false;
        };
        task_waiting_session(
            task_id,
            self.harness_focus.attached_to(),
            waiting_sessions,
            |task_id| harnesses.session_for_task(task_id),
        )
        .is_some()
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
