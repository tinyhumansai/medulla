//! Data types for the `frames` module.
#[allow(unused_imports)]
use super::*;
/// A semantic event folded out of one harness stream frame.
#[derive(Debug, Clone, PartialEq)]
pub enum StreamEvent {
    /// The harness announced its own session id (`system`/`init`).
    Session {
        /// The CLI's opaque session id.
        session_id: String,
    },
    /// A chunk of the assistant's answer text.
    AssistantDelta {
        /// The text chunk.
        text: String,
    },
    /// A chunk of the assistant's reasoning.
    ReasoningDelta {
        /// The text chunk.
        text: String,
    },
    /// The assistant invoked a tool.
    Tool {
        /// A one-line summary, `name · input`.
        label: String,
    },
    /// The turn ended. **This — not process exit — is the completion signal.**
    ///
    /// `claude` emits exactly one `result` per top-level turn, on stdout,
    /// carrying the answer and an error flag. It is top-level-only by
    /// construction, which is what makes it immune to the `SubagentStop`
    /// truncation trap that a hook-based signal falls into.
    Result {
        /// The turn's final answer text.
        reply: String,
        /// Whether the harness flagged the turn as an error. An interrupted turn
        /// arrives here too, as `subtype: error_during_execution`.
        is_error: bool,
        /// The session id, restated on the result frame.
        session_id: Option<String>,
    },
}
