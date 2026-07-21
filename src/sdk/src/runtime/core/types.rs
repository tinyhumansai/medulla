//! Data model for the core (`medulla-serve`) runtime: the shared connection
//! state the snapshot is rendered from, the connection-lifecycle enum, the
//! driver command channel vocabulary, the wire-error mirror, and the protocol
//! constants. Only data types and their trivial impls live here; the frame
//! (de)serialization and event fold live in [`super::protocol`], the async
//! connection driver in [`super::client`], and the [`Runtime`] trait surface in
//! [`super::runtime_impl`].
//!
//! [`Runtime`]: crate::runtime::Runtime

use std::time::Duration;

use serde::Deserialize;
use serde_json::Value;
use tokio::sync::oneshot;

use crate::harness_contract::HarnessStatus;
use crate::runtime::{CycleResultSummary, StreamState};
use crate::ui::chat_store::{now_millis, ChatMessage};
use crate::ui::events::{EventEnvelope, TuiEvent};

/// The NDJSON wire version this runtime speaks. The host bails on a `ready`
/// banner whose `protocol` differs (serve-protocol §3 handshake).
pub(super) const PROTOCOL_VERSION: i64 = 1;

/// How long the host waits for the `ready` banner + `hello` ack before treating
/// the connection as unavailable (serve-protocol §7, `HANDSHAKE_TIMEOUT`).
pub(super) const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(30);

/// How long the host waits for a correlated `res` to a `req` (serve-protocol §7,
/// `REQUEST_TIMEOUT`). `instruct` returns its receipt fast; the cycle itself is
/// unbounded and observed via events, so it is not under this timeout.
pub(super) const REQUEST_TIMEOUT: Duration = Duration::from_secs(60);

/// Delay between a dropped connection and the next attach attempt. Kept short —
/// the spec's 300 s `START_FAILURE_BACKOFF` governs a failed *spawn*, which this
/// attach-only milestone never does; here a drop means serve is cycling, so we
/// re-attach promptly.
pub(super) const RECONNECT_DELAY: Duration = Duration::from_millis(50);

/// Cap on retained events before the oldest are dropped.
pub(super) const EVENT_CAP: usize = 5000;
/// Cap on retained chat events before the oldest are dropped.
pub(super) const CHAT_CAP: usize = 2000;

/// The ports the host declares it can answer in `hello`. Declared eagerly so the
/// handshake mirrors the full serve capability set; actual port *hosting*
/// (answering the reverse-RPC `call` frames) is a later milestone, so inbound
/// calls are refused `port_unavailable` for now (see [`super::client`]).
pub(super) const HOST_PORTS: [&str; 10] = [
    "inference",
    "tools",
    "subagents",
    "memory",
    "context",
    "effects",
    "persistence",
    "sessions",
    "roster",
    "budgets",
];

/// The connection's lifecycle, surfaced through [`describe`](CoreState::describe)
/// and [`stream_state`](CoreState::stream_health).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum ConnState {
    /// The first attach has not completed a handshake yet.
    Connecting,
    /// Handshake succeeded; the event tap is trusted.
    Live,
    /// The socket dropped; the driver is re-attaching.
    Reconnecting,
    /// A fatal handshake outcome (version mismatch / `hello` rejection). The
    /// driver has stopped retrying; the string is the operator-facing reason.
    Unavailable(String),
}

/// A wire-error mirror (`{"code","message"}`, serve-protocol §8).
#[derive(Debug, Clone, Deserialize)]
pub(super) struct WireError {
    /// A reserved error code (`bad_request`, `not_ready`, `timeout`, …).
    pub(super) code: String,
    /// A human-readable message.
    #[serde(default)]
    pub(super) message: String,
}

/// The error surfaced from a request that failed at the protocol layer.
#[derive(Debug, Clone)]
pub(super) struct CoreError {
    /// The reserved error code.
    pub(super) code: String,
    /// A human-readable message.
    pub(super) message: String,
}

impl CoreError {
    /// A transport-level failure (socket dropped, driver gone) with no wire code.
    pub(super) fn transport(message: impl Into<String>) -> Self {
        CoreError {
            code: "internal".into(),
            message: message.into(),
        }
    }
}

impl From<WireError> for CoreError {
    fn from(e: WireError) -> Self {
        CoreError {
            code: e.code,
            message: e.message,
        }
    }
}

impl std::fmt::Display for CoreError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{} ({})", self.message, self.code)
    }
}

/// A command from a [`Runtime`](crate::runtime::Runtime) method to the async
/// connection driver. The driver mints the `req` id and correlates the `res`.
pub(super) enum Command {
    /// A request whose correlated `res` the caller awaits (`instruct`).
    Request {
        /// The `op` string (serve-protocol §4).
        op: &'static str,
        /// The `params` object.
        params: Value,
        /// Where the driver delivers the decoded result (or error).
        reply: oneshot::Sender<Result<Value, CoreError>>,
    },
    /// A fire-and-forget request whose ack the caller ignores
    /// (`answer_question`, `cancel_task`, `stop` via `abort`).
    Fire {
        /// The `op` string.
        op: &'static str,
        /// The `params` object.
        params: Value,
    },
    /// Stop the driver cleanly (`shutdown`): best-effort `stop` then exit.
    Shutdown {
        /// Signalled once the driver has stopped.
        reply: oneshot::Sender<()>,
    },
}

/// The shared connection state a [`RuntimeSnapshot`](crate::runtime::RuntimeSnapshot)
/// is rendered from. Guarded by an `Arc<Mutex<_>>`; the driver folds events into
/// it and the trait methods read it.
pub(super) struct CoreState {
    /// The serve session id (from `ready`/`hello`).
    pub(super) session_id: String,
    /// The serve build version (from `ready`), for [`describe`](CoreState::describe).
    pub(super) serve_version: Option<String>,
    /// Connection lifecycle.
    pub(super) conn: ConnState,
    /// Whether a cycle is currently running (folded from cycle framing events).
    pub(super) running: bool,
    /// Local async-mode toggle; inert server-side (no serve op backs it).
    pub(super) async_mode: bool,
    /// The folded event log, capped at [`EVENT_CAP`].
    pub(super) events: Vec<EventEnvelope>,
    /// The user/assistant/error subset, capped at [`CHAT_CAP`].
    pub(super) chat_events: Vec<EventEnvelope>,
    /// The rendered chat transcript.
    pub(super) messages: Vec<ChatMessage>,
    /// The most recent cycle's result summary, if any.
    pub(super) last_result: Option<CycleResultSummary>,
    /// The live agent-harness status, folded from `status`/harness events.
    pub(super) harness: Option<HarnessStatus>,
    /// Monotonic local sequence for [`EventEnvelope`]s.
    pub(super) seq: u64,
    /// The last protocol `event.seq` seen, for gap detection (serve-protocol §6).
    pub(super) last_stream_seq: Option<u64>,
    /// Latched when a `seq` gap was seen; cleared on a fresh connection.
    pub(super) gap: bool,
}

impl CoreState {
    /// A fresh, not-yet-connected state.
    pub(super) fn new() -> Self {
        CoreState {
            session_id: String::new(),
            serve_version: None,
            conn: ConnState::Connecting,
            running: false,
            async_mode: false,
            events: Vec::new(),
            chat_events: Vec::new(),
            messages: Vec::new(),
            last_result: None,
            harness: None,
            seq: 0,
            last_stream_seq: None,
            gap: false,
        }
    }

    /// Append `event` with a fresh local seq + timestamp, mirroring chat-visible
    /// events into the chat log and trimming both to their caps.
    pub(super) fn emit(&mut self, event: TuiEvent) {
        self.seq += 1;
        let env = EventEnvelope {
            seq: self.seq,
            at: now_millis(),
            event,
        };
        let chatty = matches!(
            env.event,
            TuiEvent::User { .. } | TuiEvent::Assistant { .. } | TuiEvent::Error { .. }
        );
        self.events.push(env.clone());
        if self.events.len() > EVENT_CAP {
            let drop = self.events.len() - EVENT_CAP;
            self.events.drain(0..drop);
        }
        if chatty {
            self.chat_events.push(env);
            if self.chat_events.len() > CHAT_CAP {
                let drop = self.chat_events.len() - CHAT_CAP;
                self.chat_events.drain(0..drop);
            }
        }
    }

    /// Record a protocol `event.seq`, latching [`gap`](CoreState::gap) when it is
    /// non-contiguous. A gap tells a full host to re-`subscribe` with `replay`;
    /// this skeleton surfaces it through [`stream_health`](CoreState::stream_health).
    pub(super) fn note_stream_seq(&mut self, seq: u64) {
        if let Some(prev) = self.last_stream_seq {
            if seq != prev + 1 {
                self.gap = true;
            }
        }
        self.last_stream_seq = Some(seq);
    }

    /// Reset the per-connection stream cursor so the first event after a
    /// (re)connect never reads as a false gap.
    pub(super) fn reset_stream_cursor(&mut self) {
        self.last_stream_seq = None;
        self.gap = false;
    }

    /// Drop the fold-derived observable state ahead of a `subscribe{replay}` on a
    /// re-attach, so serve's replayed events rebaseline it instead of stacking on
    /// top of what the previous connection already folded.
    ///
    /// Without this, replayed `cycle_*`/`instruction_queued` frames are folded a
    /// second time: cumulative counters (`usage.cycles`, `queued`) double-count
    /// and every replayed frame appends a duplicate row into `events`/`chat_events`
    /// (serve-protocol §5, `StreamState::Resyncing` rebaseline). The replay is the
    /// authoritative post-drop baseline, so the derived log/status/transcript are
    /// cleared and rebuilt from it; the negotiated identity (`session_id`,
    /// `serve_version`) and the local `async_mode` toggle are connection-spanning
    /// and left untouched.
    pub(super) fn reset_for_replay(&mut self) {
        self.running = false;
        self.events.clear();
        self.chat_events.clear();
        self.messages.clear();
        self.last_result = None;
        self.harness = None;
        self.seq = 0;
    }

    /// The event stream's health, mapped from the connection lifecycle and the
    /// gap latch (serve-protocol §6 `StreamState`).
    pub(super) fn stream_health(&self) -> StreamState {
        match &self.conn {
            ConnState::Live if self.gap => StreamState::Resyncing,
            ConnState::Live => StreamState::Live,
            ConnState::Connecting | ConnState::Reconnecting => StreamState::Resyncing,
            ConnState::Unavailable(_) => StreamState::Stalled,
        }
    }

    /// A one-line description of what backs this runtime, for the Overview.
    pub(super) fn describe(&self) -> String {
        let version = self
            .serve_version
            .as_deref()
            .map(|v| format!("medulla-serve {v}"))
            .unwrap_or_else(|| "medulla-serve".to_string());
        match &self.conn {
            ConnState::Live => format!("{version} (attached)"),
            ConnState::Connecting => format!("{version} (connecting)"),
            ConnState::Reconnecting => format!("{version} (reconnecting)"),
            ConnState::Unavailable(reason) => format!("{version} (unavailable: {reason})"),
        }
    }
}
