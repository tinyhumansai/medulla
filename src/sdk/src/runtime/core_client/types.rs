//! The core-client data model: protocol constants, the decoded event-stream
//! frame ([`CoreEvent`]), the RPC/transport error types ([`RpcError`],
//! [`CallError`]), and the per-thread sequence tracker ([`SeqTracker`]).
//!
//! These are the plain data surfaces of the NDJSON RPC client; the connection
//! logic that produces and consumes them lives in [`super::client`].

use serde_json::Value;

/// The wire protocol version this client speaks (§2). Must equal core-js's
/// `MEDULLA_PROTOCOL_VERSION`.
pub const PROTOCOL_VERSION: &str = "1";

/// 1 MiB frame cap (§1.1). An over-size frame is a protocol error.
pub const MAX_FRAME_BYTES: usize = 1024 * 1024;

/// One decoded event-stream frame (§3.2): the envelope plus the raw `event` body.
#[derive(Debug, Clone)]
pub struct CoreEvent {
    /// Monotonic per-thread sequence number used for gap detection (§3.2).
    pub seq: u64,
    /// Emit timestamp (epoch millis), or `0` when the wire omitted it.
    pub at: i64,
    /// The originating thread id.
    pub thread_id: String,
    /// The originating cycle id, folded into lane keys by the runtime.
    pub cycle_id: String,
    /// The event body `{kind, ...}` — deserialized into a `TuiEvent` by the runtime.
    pub event: Value,
}

impl CoreEvent {
    /// The event body's `kind` discriminant, or `""` when absent.
    pub fn kind(&self) -> &str {
        self.event.get("kind").and_then(Value::as_str).unwrap_or("")
    }
}

/// An RPC error body (§5.1/§5.2). `data` carries structured detail — notably the
/// `{baselineSeq, snapshot}` a `resync.required` hands back so a client can rebaseline
/// without a second round-trip.
#[derive(Debug, Clone)]
pub struct RpcError {
    /// The stable error code (e.g. `resync.required`, `thread.not-found`).
    pub code: String,
    /// A human-readable message.
    pub message: String,
    /// Whether the caller may retry the request.
    pub retryable: bool,
    /// Structured detail carried alongside the error, when present.
    pub data: Option<Value>,
}

impl std::fmt::Display for RpcError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "[{}] {} (retryable={})",
            self.code, self.message, self.retryable
        )
    }
}

/// A failed `request`: either the transport broke or the core returned an RPC error.
#[derive(Debug)]
pub enum CallError {
    /// The transport failed (socket closed, serialization error, frame too large).
    Transport(String),
    /// The core returned a structured RPC error.
    Rpc(RpcError),
}

impl std::fmt::Display for CallError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CallError::Transport(m) => write!(f, "transport: {m}"),
            CallError::Rpc(e) => write!(f, "{e}"),
        }
    }
}

impl std::error::Error for CallError {}

impl CallError {
    /// The RPC error code, when this is an RPC error (for `resync.required` etc.).
    pub fn rpc_code(&self) -> Option<&str> {
        match self {
            CallError::Rpc(e) => Some(&e.code),
            _ => None,
        }
    }
}

/// Tracks a thread's event `seq` to detect gaps (§3.2). A gap means the core
/// coalesced/dropped frames and the client should `snapshot.get` to resync.
#[derive(Debug, Clone)]
pub struct SeqTracker {
    last_seq: u64,
}

impl SeqTracker {
    /// Start from a subscribe `baselineSeq`.
    pub fn new(baseline_seq: u64) -> Self {
        SeqTracker {
            last_seq: baseline_seq,
        }
    }

    /// The highest sequence number observed so far.
    pub fn last_seq(&self) -> u64 {
        self.last_seq
    }

    /// Record an event's seq. Returns `true` when a gap was detected (this seq is
    /// beyond the next expected). Advances regardless, so the client resyncs from here.
    pub fn observe(&mut self, seq: u64) -> bool {
        let expected = self.last_seq + 1;
        let gap = seq > expected;
        if seq > self.last_seq {
            self.last_seq = seq;
        }
        gap
    }
}
