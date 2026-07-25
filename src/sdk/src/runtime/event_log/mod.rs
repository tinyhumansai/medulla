//! Shared bounded event storage for runtime conversation threads.
//!
//! Backend, core-socket, and scripted runtimes all project the same raw event
//! stream into a full log and a chat-only subset. This module owns that policy
//! so every adapter applies identical filtering and retention limits.

mod types;

pub(crate) use types::ThreadEventLog;
#[cfg(test)]
pub(crate) use types::{CHAT_CAP, EVENT_CAP};

#[cfg(test)]
mod tests;
