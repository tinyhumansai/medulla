//! Per-thread conversation history for local turns.
//!
//! # Why this exists
//!
//! A turn's `resume_session_id` promises continuity — a bounded workflow node
//! gets a fresh thread and a resumed conversation keeps its own, matching what
//! `--session-id` buys on the CLI providers. The embedded core delivered that
//! from its own session store; without one, a resumed turn built a fresh harness
//! and sent nothing but the system prompt and the new message, so the id
//! identified a conversation that no longer existed anywhere.
//!
//! This is the replacement: the transcript a turn produced is written under the
//! thread's id and replayed on the next turn that names it.
//!
//! # Why not tinyagents' session store
//!
//! It has one, behind the `sqlite` feature this workspace already enables. It is
//! also a durable *run ledger* — sessions, tool calls, telemetry, a full-text
//! search index — and the only thing needed here is "what did we say last time".
//! A JSON file per thread is inspectable, trivially removable by an operator
//! clearing a stuck conversation, and carries no schema to migrate. If a later
//! surface needs the ledger's query side, that is the moment to adopt it.
//!
//! # Layout
//!
//! `types` is the stored record, `store` the reading and writing — both
//! private, since the surface is [`load`] and [`save`] — and the trimming rule
//! below is here because both halves apply it and it is the one piece with a
//! correctness argument worth reading in one place.

mod store;
mod types;

pub use store::{load, save};

use tinyinference::message::Message;

/// Most messages replayed into a resumed turn.
///
/// A conversation grows without bound and every replayed message is prompt
/// budget spent before the model reads the new instruction. The tail is what
/// matters — recent turns carry the state a follow-up refers to — so older
/// messages are dropped rather than the whole history refused.
const MAX_REPLAYED: usize = 80;

/// Trim `messages` to at most `max`, cutting only where a provider will accept
/// the result.
///
/// # Why a raw tail is not safe
///
/// A tool exchange is three messages that must arrive together: the assistant
/// message requesting the calls, and one `Tool` message per result. Slicing at
/// an arbitrary offset can retain a result whose request was cut, and an
/// OpenAI-compatible endpoint rejects an orphaned `tool` message outright — so
/// the next resumed turn would fail on the wire rather than continue the
/// conversation.
///
/// The cut therefore advances to the first `User` message inside the window. A
/// user turn is the one boundary that never depends on preceding tool state, so
/// starting there is valid by construction rather than by case analysis.
///
/// A window with no user message at all replays nothing. That is the honest
/// answer: there is no cut point that a provider would accept, and sending a
/// malformed prefix to get *some* context would fail the turn outright.
fn trim(messages: Vec<Message>, max: usize) -> Vec<Message> {
    if messages.len() <= max && starts_cleanly(&messages) {
        return messages;
    }
    let start = messages.len().saturating_sub(max);
    let window = &messages[start..];
    match window.iter().position(|m| matches!(m, Message::User(_))) {
        Some(offset) => window[offset..].to_vec(),
        None => Vec::new(),
    }
}

/// Whether a sequence can be replayed as-is.
///
/// Only the leading edge can be malformed by trimming: a `Tool` message before
/// any assistant turn has no request to answer.
fn starts_cleanly(messages: &[Message]) -> bool {
    !matches!(messages.first(), Some(Message::Tool(_)))
}

#[cfg(test)]
mod tests;
