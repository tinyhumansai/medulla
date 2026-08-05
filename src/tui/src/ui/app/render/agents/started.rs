//! The sessions the orchestrator started, rendered under the query that caused
//! them.
//!
//! §A7 gave the orchestrator's conversation a "sessions started" block so an
//! operator could click through to a dispatched session. It was one aggregate
//! list at the top of the pane: after three queries it said *"sessions started ·
//! 7"* and left the reader to work out which query each one came from — which is
//! the fact they were looking for.
//!
//! So the entries are grouped by **turn** and drawn where that turn is:
//!
//! ```text
//! ❯ ship the auth fix and update the docs
//! ⏺ deploying two agents…
//!   ▸ t_41 · api-claude · claude × ~/proj/api · running
//!   ▸ t_42 · web-codex · codex × ~/proj/web · running
//!
//! ❯ now run the tests
//! ```
//!
//! **Attribution needs no new bookkeeping.** The conversation is an ordered
//! event stream carrying both halves already: a [`TuiEvent::User`] opens a turn,
//! and every [`TuiEvent::TaskStart`] after it — until the next `User` — is a
//! task that turn caused. The rail's own session rows carry the same task ids,
//! so joining them is a lookup rather than a side-channel.
//!
//! A session whose `task_start` is not in this stream — dispatched in another
//! thread, or folded from a snapshot that predates the visible events — is
//! **not dropped**: it is listed in a trailing group under its own heading, at
//! the end of the transcript where the reader already is. Sessions started
//! before the first query of the thread land ahead of that first query, which is
//! chronologically where they happened.

use std::collections::HashMap;

use crate::ui::agents::Line as StyledLine;
use crate::ui::events::{EventEnvelope, TuiEvent};

use super::super::super::session_focus::StartedSession;
use super::super::chat_lines;

/// The turn a task belongs to, keyed by task id.
///
/// Turn `0` is everything before the first user message — a dispatch the
/// orchestrator made on its own, or one folded in from an earlier state.
fn turn_of_tasks(events: &[EventEnvelope]) -> HashMap<String, usize> {
    let mut turns = HashMap::new();
    let mut turn = 0usize;
    for env in events {
        match &env.event {
            TuiEvent::User { .. } => turn += 1,
            TuiEvent::TaskStart { task_id, .. } => {
                // First writer wins: a task id is announced once, and a retry
                // re-announcing it belongs to the turn that first asked.
                turns.entry(task_id.clone()).or_insert(turn);
            }
            _ => {}
        }
    }
    turns
}

/// The conversation, with each turn's spawned sessions listed under it.
///
/// Returns the lines and, line for line, the task each one opens — `None` for
/// ordinary transcript lines. The parallel vector is what keeps the entries
/// clickable through the same task-keyed path as the old block: the caller
/// windows both by the same scroll offset, so a click resolves to whatever is
/// drawn at that row rather than to an index recorded before the last scroll.
pub(super) fn chat_lines_with_sessions(
    events: &[EventEnvelope],
    width: usize,
    started: &[StartedSession],
) -> (Vec<StyledLine>, Vec<Option<String>>) {
    let turns = turn_of_tasks(events);
    let mut lines: Vec<StyledLine> = Vec::new();
    let mut hits: Vec<Option<String>> = Vec::new();
    let mut push = |line: StyledLine, task: Option<String>| {
        lines.push(line);
        hits.push(task);
    };

    // The stream split at its user turns: segment `n` runs from the n-th user
    // message up to the one after it, and segment 0 is whatever came before the
    // first. Splitting is safe because `chat_lines` flushes its pending tool
    // calls immediately *before* handling a `User` event, so a boundary there
    // produces the same lines the whole stream would.
    let mut bounds = vec![0usize];
    bounds.extend(
        events
            .iter()
            .enumerate()
            .filter_map(|(index, env)| matches!(env.event, TuiEvent::User { .. }).then_some(index)),
    );
    bounds.push(events.len());

    for (turn, window) in bounds.windows(2).enumerate() {
        let (from, to) = (window[0], window[1]);
        for line in chat_lines(&events[from..to], width) {
            push(line, None);
        }
        for session in started.iter().filter(|session| {
            turns
                .get(&session.task_id)
                .is_some_and(|attributed| *attributed == turn)
        }) {
            push(entry_line(session, width), Some(session.task_id.clone()));
        }
    }

    // Whatever this stream cannot account for. Listed rather than dropped: a
    // session that is running, costing tokens and unreachable is worse than one
    // filed under a heading that admits it does not know where it came from.
    let orphans: Vec<&StartedSession> = started
        .iter()
        .filter(|session| !turns.contains_key(&session.task_id))
        .collect();
    if !orphans.is_empty() {
        push(StyledLine::default(), None);
        push(
            StyledLine {
                text: format!(
                    "sessions started outside this conversation · {}",
                    orphans.len()
                ),
                color: Some("cyan".into()),
                dim: true,
            },
            None,
        );
        for session in orphans {
            push(entry_line(session, width), Some(session.task_id.clone()));
        }
    }
    (lines, hits)
}

/// One session entry: the task, the agent, where it runs, and how it is doing.
fn entry_line(session: &StartedSession, width: usize) -> StyledLine {
    let mut parts = vec![session.task_id.clone()];
    if !session.agent.trim().is_empty() {
        parts.push(session.agent.clone());
    }
    match (&session.harness, &session.workspace) {
        (Some(harness), Some(workspace)) => parts.push(format!(
            "{harness} × {}",
            crate::ui::util::clip_left(workspace, 28)
        )),
        (Some(harness), None) => parts.push(harness.clone()),
        (None, Some(workspace)) => {
            parts.push(crate::ui::util::clip_left(workspace, 28).to_string())
        }
        (None, None) => {}
    }
    parts.push(session.status.to_string());
    StyledLine {
        text: crate::ui::util::clip(&format!("  ▸ {}", parts.join(" · ")), width.max(20)),
        color: Some("cyan".into()),
        dim: true,
    }
}
