//! Task-frame construction: turn an [`EncodeFrameInput`] into a serialized
//! `medulla-task/1` frame body ready for an encrypted message.

use crate::harness_work::WorkSnapshot;

use super::types::{EncodeFrameInput, FrameAttachments, TaskFrame, TokenUsage, MEDULLA_TASK_PROTO};

/// Build and serialize a task frame body.
pub fn encode_task_frame(input: EncodeFrameInput) -> String {
    build(input, FrameAttachments::default()).encode()
}

/// Serialize a task issued by an `agent` node within an authenticated workflow.
///
/// The marker survives loopback and remote task dispatch, allowing the worker
/// to grant OpenHuman's workflow origin to the node without confusing it with
/// an ordinary delegated task frame.
pub fn encode_workflow_node_task_frame(input: EncodeFrameInput) -> String {
    let mut frame = build(input, FrameAttachments::default());
    frame.workflow_node = true;
    frame.encode()
}

/// [`encode_task_frame`] with reported token usage (reply frames).
pub fn encode_task_frame_with_usage(input: EncodeFrameInput, usage: Option<TokenUsage>) -> String {
    build(
        input,
        FrameAttachments {
            usage,
            ..Default::default()
        },
    )
    .encode()
}

/// [`encode_task_frame`] with the child harness's work snapshot attached
/// (status and reply frames), so the orchestrator sees what the worker is
/// actually doing and not just a one-line detail.
pub fn encode_task_frame_with_work(
    input: EncodeFrameInput,
    usage: Option<TokenUsage>,
    work: Option<WorkSnapshot>,
) -> String {
    build(
        input,
        FrameAttachments {
            usage,
            work,
            session_id: None,
        },
    )
    .encode()
}

/// [`encode_task_frame`] with everything a *response* may carry — usage, the
/// work snapshot, and the session that served the task.
///
/// The entry point a worker daemon uses for its terminal frames. The others
/// remain because most senders attach nothing (a `task` frame) or only usage,
/// and naming what a frame carries at the call site is what keeps an inbound
/// request from accidentally claiming a session.
pub fn encode_task_frame_with_attachments(
    input: EncodeFrameInput,
    attachments: FrameAttachments,
) -> String {
    build(input, attachments).encode()
}

/// Assemble the frame from its input and optional attachments.
fn build(input: EncodeFrameInput, attachments: FrameAttachments) -> TaskFrame {
    TaskFrame {
        proto: MEDULLA_TASK_PROTO.to_string(),
        kind: input.kind,
        task_id: input.task_id,
        text: input.text,
        ts: input.ts,
        correlation_id: input.correlation_id,
        harness: input.harness,
        provider: input.provider,
        // Dropped when it is the provider default, so an ordinary task frame is
        // byte-identical to what a peer that predates flavors would send.
        transport: input.transport.filter(|transport| !transport.is_default()),
        custom_harness: input.custom_harness.map(String::into_boxed_str),
        model: input.model,
        tool_mode: input.tool_mode,
        workflow_node: false,
        workflow: input.workflow,
        workflow_fingerprint: input.workflow_fingerprint,
        workflow_inputs: input.workflow_inputs,
        conversation: input.conversation,
        fleet_depth: input.fleet_depth,
        usage: attachments.usage,
        // An empty snapshot says nothing and would only cost bytes on every
        // status frame, so it is dropped rather than sent.
        work: attachments
            .work
            .filter(|snapshot| !snapshot.is_empty())
            .map(Box::new),
        // Blank is not a session. A worker that never opened one must leave the
        // key absent rather than claim `""`, which downstream would record as a
        // session id nothing can resume.
        session_id: attachments
            .session_id
            .map(|id| id.trim().to_string())
            .filter(|id| !id.is_empty()),
    }
}
