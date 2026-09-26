//! The stored transcript's data model.

use tinyinference::message::Message;

/// One thread's stored transcript.
#[derive(serde::Serialize, serde::Deserialize)]
pub(super) struct Stored {
    /// The id this transcript belongs to, verbatim.
    ///
    /// Checked against the requested id on read, so a filename collision from
    /// sanitizing cannot serve one conversation's history to another.
    pub(super) thread_id: String,
    /// The messages, oldest first.
    pub(super) messages: Vec<Message>,
}
