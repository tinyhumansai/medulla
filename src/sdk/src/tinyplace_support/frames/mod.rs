//! The `medulla-tinyplace/1` task wire protocol.
//!
//! An orchestrator delegates work to remote coding agents over an encrypted
//! transport using a small JSON frame. Peers exchange `task`/`input` requests
//! and answer with `ack`/`status`/`reply`/`error`. A `capabilities` frame is the
//! request ("what can you do here?"); the answer is a distinct
//! `capabilities_result` frame carrying [`AgentCapabilities`] JSON in `text`, so
//! a result is never mistaken for a new request. Frames correlate by `task_id`
//! (cycle-scoped) and, when present, an opaque `correlation_id` (globally unique
//! per dispatch) that responders echo back verbatim.
//!
//! Split by responsibility: [`types`] holds the frame data model and its serde
//! helpers, [`encode`] builds and serializes frames, and [`decode`] parses
//! decrypted bodies and capabilities payloads. All public items are re-exported
//! here so callers use `medulla::tinyplace_support::frames::*`.

mod decode;
mod encode;
mod types;

#[cfg(test)]
mod tests;

pub use decode::{decode_task_frame, parse_agent_capabilities};
pub use encode::{encode_task_frame, encode_task_frame_with_usage};
pub use types::{
    AgentCapabilities, EncodeFrameInput, HarnessProvider, TaskFrame, TaskFrameKind, TokenUsage,
    TINYPLACE_PROTO,
};
