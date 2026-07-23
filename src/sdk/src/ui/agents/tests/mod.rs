//! Unit tests for the Agents view-model, split by responsibility: [`fold`] covers
//! the event fold and Agents-list row model; [`render`] covers status/role
//! classification, key parsing, and transcript rendering.

use crate::ui::events::{EventEnvelope, TuiEvent};

mod claims;
mod fold;
mod render;

/// Wrap an event with a synthetic monotonic sequence and timestamp.
fn env(seq: u64, event: TuiEvent) -> EventEnvelope {
    EventEnvelope {
        seq,
        at: seq as i64 * 1000,
        event,
    }
}
