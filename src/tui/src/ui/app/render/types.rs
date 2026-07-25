//! Internal data types used while rendering application events.
#[allow(unused_imports)]
use super::*;
/// Fold the chat event stream into a wrapped conversational transcript.
/// One tool call being assembled from the stream.
///
/// The name and the arguments arrive as *separate* events — `tool_call_start`
/// carries the name once, the deltas carry argument fragments — so a call can
/// only be rendered after its fragments stop arriving. Held here until the next
/// non-tool event flushes it.
#[derive(Default)]
pub(super) struct PendingCall {
    pub(super) name: String,
    pub(super) args: String,
}
