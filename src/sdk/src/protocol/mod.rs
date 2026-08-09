//! Medulla's own wire protocol + agent-runtime layer for the medulla TUI/daemon.
//!
//! Everything medulla puts on a wire lives here:
//!
//! - [`frames`] — the `medulla-task/1` task frame protocol (delegated work
//!   over encrypted DMs).
//! - [`control`] — owner→machine harness control frames (session-targeted input).
//! - [`screen`] — the `medulla.screen.v1` protocol, streaming a worker's live
//!   terminal to a watching orchestrator as mosh-style synchronised state.
//! - [`envelope`] — the harness session-envelope wire model (v1 and v2).
//! - [`consumer`] — receiver-side fold of the v2 harness stream into a live
//!   [`consumer::SessionView`].
//! - [`status`] — the derived session-status state machine over harness events.
//! - [`service`] — the background host-link observation the TUI renders.

pub mod consumer;
pub mod control;
pub mod env;
pub mod envelope;
pub mod frames;
pub mod screen;
pub mod service;
pub mod status;
pub mod system_info;

pub use consumer::{
    apply_session_envelope, fold_session_envelopes, initial_session_view, parse_session_envelope,
    FeedEntry, SessionView, SessionViewLimits, ToolActivity, DEFAULT_LIMITS,
};
pub use control::{
    encode_harness_control_frame, parse_harness_control_frame, HarnessControlFrame,
    HARNESS_CONTROL_VERSION,
};
pub use frames::{
    decode_task_frame, dispatchable_flavors, encode_task_frame, encode_task_frame_with_attachments,
    encode_task_frame_with_usage, encode_task_frame_with_work, encode_workflow_node_task_frame,
    parse_agent_capabilities, AgentCapabilities, BudgetSource, BudgetWindow, CustomHarnessAdvert,
    EncodeFrameInput, FrameAttachments, HarnessBudget, HarnessProvider, HarnessReadiness,
    HarnessTransport, TaskFrame, TaskFrameKind, TokenUsage, WorkflowAdvert, WorkflowInputAdvert,
    CODEX_SERVER_FLAVOR, MEDULLA_TASK_PROTO,
};
pub use screen::{
    apply_frame, build_frame, changed_rows, coalesce_runs, encode_screen_message,
    parse_screen_message, ApplyOutcome, Color, FrameDecision, RowUpdate, RunStyle, ScreenFrame,
    ScreenGrid, ScreenMessage, ScreenRun, ScreenView, ATTR_BOLD, ATTR_INVERSE, ATTR_ITALIC,
    ATTR_UNDERLINE, SCREEN_PROTO,
};
pub use status::{
    initial_status, reduce_status, tick_status, SemanticEvent, SessionStatusState, StatusStep,
    DEFAULT_IDLE_AFTER_MS, STATE_ERRORED, STATE_IDLE, STATE_RUNNING, STATE_RUNNING_TOOL,
    STATE_STOPPED, STATE_WAITING_APPROVAL,
};
pub use system_info::{capture_system_info, WorkerSystemInfo};

// The harness session-envelope + typed-event wire model, owned here. Re-exported
// so callers work with the same types the fold and status machine operate on.
// `envelope::HarnessProvider` is intentionally NOT re-exported (it is a bare
// `String`); [`frames::HarnessProvider`] is this module's typed provider for the
// task-frame protocol.
pub use envelope::{
    AnySessionEnvelope, ApprovalRequestPayload, ErrorPayload, HarnessBucket, HarnessBucketUnit,
    HarnessEnvelopeScope, HarnessEvent, HarnessEventKind, HarnessEventRole, HarnessInfo,
    HarnessMessage, HarnessMessageRole, HarnessScope, HarnessSessionState, HarnessSource,
    HarnessToolKind, LifecyclePayload, SessionEnvelope, SessionEnvelopeV1, SessionEnvelopeV2,
    StatusPayload, TextPayload, ToolCallPayload, ToolResultPayload, UnknownPayload,
    UserPromptPayload, SESSION_ENVELOPE_VERSION_V1, SESSION_ENVELOPE_VERSION_V2,
};
