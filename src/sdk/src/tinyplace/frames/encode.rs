//! Task-frame construction: turn an [`EncodeFrameInput`] into a serialized
//! `medulla-tinyplace/1` frame body ready for an encrypted message.

use super::types::{EncodeFrameInput, TaskFrame, TokenUsage, TINYPLACE_PROTO};

/// Build and serialize a task frame body.
pub fn encode_task_frame(input: EncodeFrameInput) -> String {
    encode_task_frame_with_usage(input, None)
}

/// [`encode_task_frame`] with reported token usage (reply frames).
pub fn encode_task_frame_with_usage(input: EncodeFrameInput, usage: Option<TokenUsage>) -> String {
    TaskFrame {
        proto: TINYPLACE_PROTO.to_string(),
        kind: input.kind,
        task_id: input.task_id,
        text: input.text,
        ts: input.ts,
        correlation_id: input.correlation_id,
        harness: input.harness,
        provider: input.provider,
        model: input.model,
        usage,
    }
    .encode()
}
