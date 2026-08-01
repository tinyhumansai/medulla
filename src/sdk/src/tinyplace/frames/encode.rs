//! Task-frame construction: turn an [`EncodeFrameInput`] into a serialized
//! `medulla-tinyplace/1` frame body ready for an encrypted message.

use crate::harness_work::WorkSnapshot;

use super::types::{EncodeFrameInput, TaskFrame, TokenUsage, TINYPLACE_PROTO};

/// Build and serialize a task frame body.
pub fn encode_task_frame(input: EncodeFrameInput) -> String {
    build(input, None, None).encode()
}

/// [`encode_task_frame`] with reported token usage (reply frames).
pub fn encode_task_frame_with_usage(input: EncodeFrameInput, usage: Option<TokenUsage>) -> String {
    build(input, usage, None).encode()
}

/// [`encode_task_frame`] with the child harness's work snapshot attached
/// (status and reply frames), so the orchestrator sees what the worker is
/// actually doing and not just a one-line detail.
pub fn encode_task_frame_with_work(
    input: EncodeFrameInput,
    usage: Option<TokenUsage>,
    work: Option<WorkSnapshot>,
) -> String {
    build(input, usage, work).encode()
}

/// Assemble the frame from its input and optional attachments.
fn build(
    input: EncodeFrameInput,
    usage: Option<TokenUsage>,
    work: Option<WorkSnapshot>,
) -> TaskFrame {
    TaskFrame {
        proto: TINYPLACE_PROTO.to_string(),
        kind: input.kind,
        task_id: input.task_id,
        text: input.text,
        ts: input.ts,
        correlation_id: input.correlation_id,
        harness: input.harness,
        provider: input.provider,
        custom_harness: input.custom_harness.map(String::into_boxed_str),
        model: input.model,
        tool_mode: input.tool_mode,
        workflow: input.workflow,
        conversation: input.conversation,
        fleet_depth: input.fleet_depth,
        usage,
        // An empty snapshot says nothing and would only cost bytes on every
        // status frame, so it is dropped rather than sent.
        work: work.filter(|snapshot| !snapshot.is_empty()).map(Box::new),
    }
}
