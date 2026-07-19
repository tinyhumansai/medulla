//! Folding backend events into local thread state: the behaviour-heavy
//! [`State`] impl that pushes optimistic turns, de-duplicates echoes, and folds
//! each backend `EventEnvelope` into a thread's event log, plus the mappers that
//! translate the client's protocol vocabulary onto the TUI's.

use std::collections::HashMap;

use serde_json::Value;

use crate::client::{EventEnvelope as ClientEnvelope, EventKind, SessionSummary};
use crate::runtime::{CycleResultSummary, ThreadSummary};
use crate::ui::chat_store::{iso8601_utc, now_millis, ChatMessage, MainChatSummary};
use crate::ui::events::{EventEnvelope, TuiEvent};

use super::types::{State, Thread, CHAT_CAP, EVENT_CAP};

impl State {
    /// Push a fully-formed local event into a thread, applying both caps and the
    /// chat-events subset filter.
    pub(super) fn push_event(thread: &mut Thread, env: EventEnvelope) {
        let chatty = matches!(
            env.event,
            TuiEvent::User { .. } | TuiEvent::Assistant { .. } | TuiEvent::Error { .. }
        );
        thread.events.push(env.clone());
        if thread.events.len() > EVENT_CAP {
            let drop = thread.events.len() - EVENT_CAP;
            thread.events.drain(0..drop);
        }
        if chatty {
            thread.chat_events.push(env);
            if thread.chat_events.len() > CHAT_CAP {
                let drop = thread.chat_events.len() - CHAT_CAP;
                thread.chat_events.drain(0..drop);
            }
        }
    }

    /// Optimistically append the just-submitted user turn to the active thread.
    pub(super) fn push_local_user(&mut self, thread_id: &str, body: &str, at: i64) {
        self.seq += 1;
        let seq = self.seq;
        if let Some(t) = self.by_id(thread_id) {
            t.running = true;
            t.pending_user_echo = Some(body.to_string());
            t.messages.push(ChatMessage {
                role: "user".into(),
                content: body.to_string(),
            });
            Self::push_event(
                t,
                EventEnvelope {
                    seq,
                    at,
                    event: TuiEvent::User {
                        body: body.to_string(),
                    },
                },
            );
        }
    }

    /// Append a locally-sourced error (e.g. a failed send) and clear running.
    pub(super) fn push_local_error(&mut self, thread_id: &str, source: &str, message: &str) {
        self.seq += 1;
        let seq = self.seq;
        if let Some(t) = self.by_id(thread_id) {
            t.running = false;
            Self::push_event(
                t,
                EventEnvelope {
                    seq,
                    at: now_millis(),
                    event: TuiEvent::Error {
                        source: source.into(),
                        message: message.into(),
                    },
                },
            );
        }
    }

    /// Fold one backend event into the thread bound to `session_id`. Returns the
    /// local seq assigned, or `None` when the event was de-duplicated or the
    /// thread is gone.
    pub(super) fn fold(&mut self, session_id: &str, env: &ClientEnvelope) -> Option<u64> {
        let kind = env.kind();
        // De-duplicate the echo of an optimistically-appended user turn.
        if let EventKind::User { body } = &kind {
            if let Some(t) = self.by_session(session_id) {
                if t.pending_user_echo.as_deref() == Some(body.as_str()) {
                    t.pending_user_echo = None;
                    return None;
                }
            }
        }

        self.seq += 1;
        let seq = self.seq;
        let at = env.at as i64;
        let event = map_event(kind);

        let t = self.by_session(session_id)?;
        match &event {
            TuiEvent::User { body } => t.messages.push(ChatMessage {
                role: "user".into(),
                content: body.clone(),
            }),
            TuiEvent::Assistant { body } => t.messages.push(ChatMessage {
                role: "assistant".into(),
                content: body.clone(),
            }),
            TuiEvent::CycleEnd { pass_count, .. } => {
                t.running = false;
                t.last_result = Some(CycleResultSummary {
                    pass_count: *pass_count,
                    task_ledger: HashMap::new(),
                });
            }
            TuiEvent::CycleStart { .. } => t.running = true,
            _ => {}
        }
        Self::push_event(t, EventEnvelope { seq, at, event });
        Some(seq)
    }

    /// One [`ThreadSummary`] per thread, with `attention` counting folded errors.
    pub(super) fn thread_summaries(&self) -> Vec<ThreadSummary> {
        self.threads
            .iter()
            .map(|t| {
                let mut attention = 0usize;
                for env in &t.events {
                    if matches!(env.event, TuiEvent::Error { .. }) {
                        attention += 1;
                    }
                }
                ThreadSummary {
                    id: t.id.clone(),
                    parent_id: t.parent_id.clone(),
                    name: t.name.clone(),
                    running: t.running,
                    turns: t.messages.len().div_ceil(2),
                    running_tasks: 0,
                    attention,
                }
            })
            .collect()
    }
}

/// Map a client [`EventKind`] onto the TUI's [`TuiEvent`] vocabulary.
fn map_event(kind: EventKind) -> TuiEvent {
    match kind {
        EventKind::User { body } => TuiEvent::User { body },
        EventKind::Assistant { body } => TuiEvent::Assistant { body },
        EventKind::CycleStart { cycle_id } => TuiEvent::CycleStart {
            cycle_id: cycle_id.unwrap_or_default(),
        },
        EventKind::CycleEnd {
            cycle_id,
            pass_count,
            duration_ms,
            ..
        } => TuiEvent::CycleEnd {
            cycle_id: cycle_id.unwrap_or_default(),
            pass_count: pass_count.unwrap_or(0) as i64,
            duration_ms: duration_ms.unwrap_or(0) as i64,
        },
        EventKind::Error { source, message } => TuiEvent::Error { source, message },
        EventKind::AssistantDelta { delta } => TuiEvent::AssistantDelta { delta },
        EventKind::ReasoningDelta { delta } => TuiEvent::ReasoningDelta { delta },
        EventKind::ToolCallDelta { value } => TuiEvent::ToolCallDelta {
            index: value.get("index").and_then(Value::as_i64).unwrap_or(0),
            args_delta: value
                .get("argsDelta")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string(),
        },
        EventKind::Unknown(v) => TuiEvent::Unknown {
            kind: v
                .get("kind")
                .and_then(Value::as_str)
                .unwrap_or("unknown")
                .to_string(),
            data: v.as_object().cloned().unwrap_or_default(),
        },
    }
}

/// Map a backend session summary onto a resume-picker row. `turns` approximates
/// `lastSeq / 2` (one user + one assistant per turn).
pub(super) fn summary_from_session(s: &SessionSummary) -> MainChatSummary {
    let name = s
        .title
        .clone()
        .filter(|t| !t.is_empty())
        .unwrap_or_else(|| s.session_id.clone());
    let turns = s.last_seq.unwrap_or(0).max(0) as usize / 2;
    MainChatSummary {
        session_id: s.session_id.clone(),
        name,
        turns,
        thread_count: 1,
        updated_at: iso8601_utc(s.last_active_at.unwrap_or(0)),
    }
}
