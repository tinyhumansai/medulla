//! The TUI event vocabulary: every library `CycleEvent` plus the host-sourced
//! rows (cycle framing, conversation turns, agent/session status, effects).
//! `TuiEvent` deserializes any JSON `{kind, ...}` shape, keeping unknown kinds
//! as a passthrough so a newer backend never drops rows on an older TUI.
//!
//! The module is split by responsibility: [`types`] holds the event data model
//! ([`TuiEvent`] and its payload structs, plus [`EventEnvelope`]), [`serde_impl`]
//! the custom compact-JSON `Serialize`/`Deserialize`, and [`derive`] the
//! read-only derivations ([`TuiEvent::kind`], [`chat_transcript`],
//! [`last_assistant_message`], [`describe_event`]). All public items are
//! re-exported here so callers use `medulla::ui::events::*`.

mod derive;
mod serde_impl;
mod types;

#[cfg(test)]
mod tests;

pub use derive::{chat_transcript, describe_event, last_assistant_message};
pub use types::{EventEnvelope, NodeTrace, TaskDigest, ToolCall, TuiEvent, Usage};
