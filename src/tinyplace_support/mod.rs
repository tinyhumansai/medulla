//! tinyplace protocol + agent-runtime layer for the medulla TUI/daemon.
//!
//! Session envelopes and harness event types are **not** hand-rolled here — they
//! are re-exported from the published `tinyplace` Rust SDK
//! (the SDK `tinyplace::types` module), so this module and the SDK share one wire
//! model. What lives here is the medulla-specific protocol the SDK does not
//! carry, plus thin async helpers over the SDK client:
//!
//! - [`frames`] — the `medulla-tinyplace/1` task frame protocol (delegated work
//!   over encrypted DMs).
//! - [`control`] — owner→machine harness control frames (session-targeted input).
//! - [`consumer`] — receiver-side fold of the SDK's v2 harness stream into a live
//!   [`consumer::SessionView`].
//! - [`status`] — the derived session-status state machine over SDK events.
//! - [`config`] — the tinyplace CLI config-file model and endpoint resolution.
//! - [`runtime`] — agent-runtime helpers: a file-backed [`runtime::FileSessionStore`],
//!   identity bootstrap, and async mailbox / contact / presence loops driving the
//!   SDK client.

pub mod config;
pub mod consumer;
pub mod control;
pub mod env;
pub mod frames;
pub mod runtime;
pub mod service;
pub mod status;

/// The published tinyplace Rust SDK, re-exported so downstream code depends on a
/// single tinyplace surface.
pub use tinyplace;

pub use config::{
    config_path, load_config, parse_config, resolve_endpoint, write_config, TinyPlaceConfig,
    DEFAULT_ENDPOINT,
};
pub use consumer::{
    apply_session_envelope, fold_session_envelopes, initial_session_view, parse_session_envelope,
    FeedEntry, SessionView, SessionViewLimits, ToolActivity, DEFAULT_LIMITS,
};
pub use control::{
    encode_harness_control_frame, parse_harness_control_frame, HarnessControlFrame,
    HARNESS_CONTROL_VERSION,
};
pub use frames::{
    decode_task_frame, encode_task_frame, encode_task_frame_with_usage, parse_agent_capabilities,
    AgentCapabilities, EncodeFrameInput, HarnessProvider, TaskFrame, TaskFrameKind, TokenUsage,
    TINYPLACE_PROTO,
};
pub use runtime::{
    load_or_create_identity, spawn_contact_auto_accepter, spawn_mailbox_poll,
    spawn_presence_heartbeat, FileSessionStore, MailboxItem, MailboxPoll, RuntimeError,
    RuntimeResult,
};
pub use status::{
    initial_status, reduce_status, tick_status, SemanticEvent, SessionStatusState, StatusStep,
    DEFAULT_IDLE_AFTER_MS, STATE_ERRORED, STATE_IDLE, STATE_RUNNING, STATE_RUNNING_TOOL,
    STATE_STOPPED, STATE_WAITING_APPROVAL,
};

// Harness session-envelope + typed-event model, owned by the SDK. Re-exported so
// callers work with the same types the fold and status machine operate on.
// `HarnessProvider` is intentionally NOT re-exported from the SDK (there it is a
// bare `String`); [`frames::HarnessProvider`] is this module's typed provider for
// the task-frame protocol.
pub use tinyplace::types::{
    AnySessionEnvelope, ApprovalRequestPayload, ErrorPayload, HarnessBucket, HarnessBucketUnit,
    HarnessEnvelopeScope, HarnessEvent, HarnessEventKind, HarnessEventRole, HarnessInfo,
    HarnessMessage, HarnessMessageRole, HarnessScope, HarnessSessionState, HarnessSource,
    HarnessToolKind, LifecyclePayload, SessionEnvelope, SessionEnvelopeV1, SessionEnvelopeV2,
    StatusPayload, TextPayload, ToolCallPayload, ToolResultPayload, UnknownPayload,
    UserPromptPayload, SESSION_ENVELOPE_VERSION_V1, SESSION_ENVELOPE_VERSION_V2,
};
