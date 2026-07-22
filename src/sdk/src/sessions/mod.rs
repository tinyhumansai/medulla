//! Interactive coding-agent session management: the two lifetime classes, the
//! two turn-source drivers, and the machinery that runs them.
//!
//! # The two axes
//!
//! A session is described by two **independent** choices. Conflating them is the
//! mistake this module exists to prevent.
//!
//! - [`SessionClass`] — *how long does it live?*
//!   [`Bounded`](SessionClass::Bounded) is task-scoped: created for one task
//!   frame, one turn, torn down on the reply.
//!   [`Unbound`](SessionClass::Unbound) is long-lived: the operator can attach
//!   and converse across turns.
//! - [`SessionDriver`] — *where do its turns come from?* [`Task`](SessionDriver::Task)
//!   means `medulla-tinyplace/1` frames; [`Envelope`](SessionDriver::Envelope)
//!   means `tinyplace.harness.session.v*` packets from a wrapper.
//!
//! Every combination is meaningful, and the seam between the drivers is
//! [`input::fold`] — the single place that knows the difference. Everything
//! downstream sees only a normalized [`TurnRequest`].
//!
//! A third, *derived* choice — [`Transport`](routing::Transport) — follows from
//! the class and the provider: an unbound `claude` session gets a live
//! [`interactive`] process, everything else runs one-shot and rebuilds context
//! from a captured session id.
//!
//! # Naming warning
//!
//! **`Bounded`/`Unbound` here are the inverse of the JavaScript prior art's
//! vocabulary.** The former `core-js` orchestrator (since removed from this
//! repo) and the tiny.place TypeScript SDK daemon call the long-lived session
//! "bound" (bound to a thread) and the throwaway one "pool"/"unbound". Here the
//! adjective describes the *lifetime*. When reading that prior art, translate:
//! their `bound` is our [`SessionClass::Unbound`].
//!
//! # Layout
//!
//! - [`types`] — the data model.
//! - [`routing`] — class and transport routing, and the provider capability matrix.
//! - [`registry`] — session-id bindings, LRU eviction, and per-conversation
//!   turn serialization.
//! - [`input`] — **the driver seam**: task frames and session envelopes folded
//!   into one normalized turn.
//! - [`completion`] — when an *interactive* turn is finished, read from the
//!   harness's own transcript rather than inferred from silence.
//! - [`turn_stream`] — the mode-independent fold: raw harness lines into the
//!   semantic events a peer is shown, plus the reply. Shared so the two modes
//!   cannot drift in what they report.
//! - [`interactive`] — the live stdin/stdout turn transport.
//! - [`manager`] — [`SessionManager`], the surface the TUI and daemon drive.
//! - [`ops`] — [`SessionOp`], the operator actions the Sessions screen dispatches.

pub mod completion;
pub mod input;
pub mod interactive;
pub mod manager;
pub mod ops;
pub mod registry;
pub mod routing;
pub mod turn_stream;
pub mod types;

#[cfg(test)]
mod tests;

pub use completion::{TurnSignal, TurnWatcher};
pub use input::{fold, fold_envelope, Folded, Observation, SessionInput};
pub use interactive::{InteractiveSession, InteractiveSpec, StreamEvent};
pub use manager::{OpenSession, SessionConfig, SessionManager, TranscriptLine, TranscriptRole};
pub use ops::SessionOp;
pub use registry::{SessionRegistry, TurnPlan, DEFAULT_MAX_BINDINGS};
pub use routing::{
    can_resume, can_run_interactive, has_continuity, route_session_class, route_transport,
    Stimulus, Transport,
};
pub use turn_stream::{LineFold, TurnStream};
pub use types::{
    SessionClass, SessionDriver, SessionKey, SessionPhase, SessionPolicy, SessionRecord,
    TurnOrigin, TurnOutcome, TurnRequest,
};
