//! The session data model: lifetime class, turn-source driver, routing policy,
//! the per-session record the UI renders, and the turn request/outcome pair.
//!
//! Only data and trivial `impl`s live here. Routing lives in
//! [`super::routing`], the binding registry in [`super::registry`], the
//! normalized turn seam in [`super::input`], and the live process transport in
//! [`super::interactive`].

use std::fmt;

use serde::{Deserialize, Serialize};

use crate::protocol::HarnessProvider;

/// How long a coding-agent session lives, and therefore whose context it carries.
///
/// # Naming
///
/// **This is the inverse of the JavaScript prior art's vocabulary, and the
/// inversion is deliberate.** The implementations this was ported from — the
/// former `core-js` orchestrator (since removed from this repo) and the
/// tiny.place daemon — call the *long-lived* session "bound" (it
/// is bound to a thread) and the throwaway one "unbound"/"pool". Here the
/// adjective describes the session's **lifetime**, not its attachment: a
/// [`SessionClass::Bounded`] session has a bounded life (one turn), a
/// [`SessionClass::Unbound`] session has an unbounded one. When reading that
/// prior art, translate: their `bound` is our [`SessionClass::Unbound`], their
/// `pool` is our [`SessionClass::Bounded`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SessionClass {
    /// Task-scoped. A session is created for one task frame, runs exactly one
    /// turn, and is torn down when the reply is sent. Two bounded sessions never
    /// see each other's context, so they may run concurrently without
    /// serialization.
    Bounded,
    /// Long-lived. The session outlives any single turn; an operator (or a peer)
    /// can attach and converse across turns. Turns on one unbound session must be
    /// serialized — two concurrent turns would interleave onto one transcript.
    Unbound,
}

impl SessionClass {
    /// The wire/display string.
    pub fn as_str(self) -> &'static str {
        match self {
            SessionClass::Bounded => "bounded",
            SessionClass::Unbound => "unbound",
        }
    }

    /// Whether turns on this class must be serialized against each other.
    ///
    /// Load-bearing, not a nicety: two concurrent turns resuming one session
    /// interleave onto a single transcript, and two concurrent *first* turns each
    /// start a session and race to bind, silently orphaning one.
    pub fn serializes(self) -> bool {
        matches!(self, SessionClass::Unbound)
    }

    /// Whether this class survives the turn that created it.
    pub fn is_persistent(self) -> bool {
        matches!(self, SessionClass::Unbound)
    }
}

impl fmt::Display for SessionClass {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Who *started* a session — its provenance, fixed for the session's whole life.
///
/// # Origin is not ownership
///
/// This is deliberately **not** "who may drive it now". That is *control*
/// (`HarnessControl` in the app crate, `owner` in the spec), it moves at runtime,
/// and the two answer different questions:
///
/// | | Origin | Control / owner |
/// |---|---|---|
/// | Question | who started this session | who may start a turn in it *now* |
/// | Lifetime | immutable | moves on every take / hand-back |
/// | Set by | the creation path | takeover, hand-back, spawn-time choice |
///
/// So an [`Orchestrator`](Self::Orchestrator)-originated session an operator
/// takes over is still `origin: orchestrator` while they hold it, and still is
/// after they hand it back. Nothing may write this field after creation — the
/// structures that carry it put it in their immutable half for that reason.
///
/// Origin is what the two rail groups ("agent task sublanes" and "your
/// harnesses") collapse into: both are sessions of an agent, and this is the
/// only thing that differs between them.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SessionOrigin {
    /// Auto-created to serve a dispatched task. Unnamed: the UI labels it from
    /// the task it was created for.
    Orchestrator,
    /// Spun up by a person from the UI. Carries the name they gave it.
    User,
}

impl SessionOrigin {
    /// The wire/display string.
    pub fn as_str(self) -> &'static str {
        match self {
            SessionOrigin::Orchestrator => "orchestrator",
            SessionOrigin::User => "user",
        }
    }

    /// Whether a person, rather than a dispatch, started this session.
    ///
    /// Display and labelling only — never a gate on dispatch. Control is the
    /// gate: a user-originated session that was handed to the orchestrator is
    /// dispatchable, and an orchestrator-originated one a person holds is not.
    pub fn is_user(self) -> bool {
        matches!(self, SessionOrigin::User)
    }

    /// Whether a dispatch, rather than a person, started this session.
    pub fn is_orchestrator(self) -> bool {
        matches!(self, SessionOrigin::Orchestrator)
    }
}

impl fmt::Display for SessionOrigin {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// The operator's routing preference, resolved against the stimulus by
/// [`route_session_class`](super::routing::route_session_class).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SessionPolicy {
    /// Let the stimulus decide: task frames route bounded, conversational
    /// plain-text routes unbound.
    #[default]
    Auto,
    /// Pin every session to [`SessionClass::Bounded`].
    Bounded,
    /// Pin every session to [`SessionClass::Unbound`].
    Unbound,
}

impl SessionPolicy {
    /// The wire/display string.
    pub fn as_str(self) -> &'static str {
        match self {
            SessionPolicy::Auto => "auto",
            SessionPolicy::Bounded => "bounded",
            SessionPolicy::Unbound => "unbound",
        }
    }

    /// Parse a policy name; unknown names fall back to [`SessionPolicy::Auto`]
    /// so a newer peer's value never wedges an older daemon.
    pub fn parse(value: &str) -> SessionPolicy {
        match value.trim().to_ascii_lowercase().as_str() {
            "bounded" | "pool" | "task" => SessionPolicy::Bounded,
            "unbound" | "conversation" | "interactive" => SessionPolicy::Unbound,
            _ => SessionPolicy::Auto,
        }
    }
}

/// What *drives* a session's turns. Orthogonal to [`SessionClass`]: the class
/// decides how long the session lives, the driver decides where its turns come
/// from. Every combination is meaningful.
///
/// This is the seam the whole module exists to make explicit — see
/// [`SessionInput`](super::input::SessionInput), the normalized form both
/// drivers fold into.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SessionDriver {
    /// Turns arrive as `medulla-task/1` task frames: a `task` frame opens a
    /// turn, `input` frames steer the in-flight one, and the daemon answers with
    /// `status`/`reply`/`error`.
    Task,
    /// Turns arrive as `tinyplace.harness.session.v*` envelopes streamed from a
    /// wrapper: a `user_prompt` event opens a turn and the agent events that
    /// follow report it. The transcript is authoritative; the daemon observes.
    Envelope,
}

impl SessionDriver {
    /// The wire/display string.
    pub fn as_str(self) -> &'static str {
        match self {
            SessionDriver::Task => "task",
            SessionDriver::Envelope => "envelope",
        }
    }
}

impl fmt::Display for SessionDriver {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// The identity a session's continuity is keyed on: who is talking, and to which
/// harness.
///
/// Bindings are **per provider**: the same peer on `claude` and on `codex` holds
/// two independent sessions, because a session id from one CLI is meaningless to
/// the other.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct SessionKey {
    /// The conversation anchor — a peer's tiny.place cryptoId, or a local label
    /// for an operator-opened session.
    pub conversation: String,
    /// Which coding-agent CLI serves this conversation.
    pub provider: HarnessProvider,
}

impl SessionKey {
    /// Build a key from a conversation anchor and provider.
    pub fn new(conversation: impl Into<String>, provider: HarnessProvider) -> Self {
        SessionKey {
            conversation: conversation.into(),
            provider,
        }
    }

    /// The registry's map key. Provider first, separated by a space, so a lookup
    /// is an exact match and never a suffix scan — resetting `bob` must not wipe
    /// a conversation whose anchor happens to end in `bob`.
    pub fn map_key(&self) -> String {
        format!("{} {}", self.provider.as_str(), self.conversation)
    }
}

impl fmt::Display for SessionKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}·{}", self.provider.as_str(), self.conversation)
    }
}

/// Where a session is in its lifecycle.
///
/// A [`SessionClass::Bounded`] session only ever passes through
/// `Starting → Turn → Closed`; an unbound one idles in [`SessionPhase::Live`]
/// between turns and is the only class that can be attached to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SessionPhase {
    /// Registered but no process started. Costs nothing; a handle only.
    Idle,
    /// The child process is being spawned.
    Starting,
    /// The process is up and waiting for a turn.
    Live,
    /// A turn is in flight.
    Turn,
    /// An interrupt was sent; draining the terminating frame.
    Interrupting,
    /// The process has exited (cleanly or not) and the session is finished.
    Closed,
    /// The session failed to start or died mid-turn; `last_error` says why.
    Failed,
}

impl SessionPhase {
    /// The wire/display string.
    pub fn as_str(self) -> &'static str {
        match self {
            SessionPhase::Idle => "idle",
            SessionPhase::Starting => "starting",
            SessionPhase::Live => "live",
            SessionPhase::Turn => "turn",
            SessionPhase::Interrupting => "interrupting",
            SessionPhase::Closed => "closed",
            SessionPhase::Failed => "failed",
        }
    }

    /// A single-width glyph for dense list rendering.
    pub fn glyph(self) -> char {
        match self {
            SessionPhase::Idle => '·',
            SessionPhase::Starting => '◌',
            SessionPhase::Live => '●',
            SessionPhase::Turn => '▶',
            SessionPhase::Interrupting => '◐',
            SessionPhase::Closed => '✓',
            SessionPhase::Failed => '✕',
        }
    }

    /// Whether the session has reached a terminal phase.
    pub fn is_terminal(self) -> bool {
        matches!(self, SessionPhase::Closed | SessionPhase::Failed)
    }

    /// Whether a new turn may be submitted right now.
    pub fn accepts_turn(self) -> bool {
        matches!(self, SessionPhase::Idle | SessionPhase::Live)
    }
}

impl fmt::Display for SessionPhase {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// The operator-facing snapshot of one session, as rendered in the Sessions tab.
///
/// Every field is a projection of manager state; nothing here is authoritative
/// storage.
#[derive(Debug, Clone, PartialEq)]
pub struct SessionRecord {
    /// The manager's stable local handle (`s_…`), used for attach/submit/close.
    /// Distinct from [`SessionRecord::harness_session_id`] — that one is the
    /// CLI's own id and is observability-only.
    pub id: String,
    /// Whose conversation this is, and on which provider.
    pub key: SessionKey,
    /// Who started this session. Immutable — see [`SessionOrigin`]; it is *not*
    /// who holds the session now.
    pub origin: SessionOrigin,
    /// The display name a person gave this session when they spun it up.
    ///
    /// `None` for an [`SessionOrigin::Orchestrator`]-originated session, which
    /// the UI labels from the task it was created for instead.
    pub name: Option<String>,
    /// Lifetime class.
    pub class: SessionClass,
    /// Where this session's turns come from.
    pub driver: SessionDriver,
    /// Lifecycle phase.
    pub phase: SessionPhase,
    /// The working directory the child runs in.
    pub workspace: String,
    /// The wrapped CLI's own session id (claude `session_id`, codex
    /// `thread_id`), captured from its stream.
    ///
    /// **Observability only.** It is in-memory, LRU-evicted, dies with the
    /// process, and is never shown to the operator as a resume handle nor used
    /// as a durable key.
    pub harness_session_id: Option<String>,
    /// How many turns have completed on this session.
    pub turns: u64,
    /// Epoch ms when the session was registered.
    pub created_at: i64,
    /// Epoch ms of the most recent activity — a liveness hint for the detail pane.
    pub last_at: i64,
    /// The most recent failure, kept sticky so a recovered session still shows
    /// what went wrong.
    pub last_error: Option<String>,
}

impl SessionRecord {
    /// Milliseconds since the last activity, given a clock reading.
    pub fn idle_ms(&self, now: i64) -> i64 {
        now.saturating_sub(self.last_at).max(0)
    }

    /// Whether an operator may attach and converse with this session.
    ///
    /// Only unbound sessions are attachable: a bounded one is torn down on its
    /// reply, so there is nothing to converse with.
    pub fn is_attachable(&self) -> bool {
        self.class == SessionClass::Unbound && !self.phase.is_terminal()
    }
}

/// Why a turn is being submitted — the provenance that survives the
/// [`SessionInput`](super::input::SessionInput) normalization.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TurnOrigin {
    /// A `medulla-task/1` task frame from a peer.
    Frame {
        /// The frame's cycle-scoped task id.
        task_id: String,
        /// The dispatch key responders echo verbatim, when the sender set one.
        correlation_id: Option<String>,
    },
    /// A harness session envelope (`user_prompt`).
    Envelope {
        /// The envelope event's idempotency id.
        event_id: String,
        /// The envelope event's monotonic per-session sequence.
        seq: i64,
    },
    /// The operator typed it into the Sessions tab.
    Operator,
}

impl TurnOrigin {
    /// The wire/display string for the origin's kind.
    pub fn as_str(&self) -> &'static str {
        match self {
            TurnOrigin::Frame { .. } => "frame",
            TurnOrigin::Envelope { .. } => "envelope",
            TurnOrigin::Operator => "operator",
        }
    }

    /// The provenance a session auto-created for this turn is born with.
    ///
    /// A task frame is a dispatch, so the session it creates is the
    /// orchestrator's (§4.1: a task without a session id creates one). Anything
    /// a person or a wrapper they are sitting in front of drives is theirs — an
    /// envelope stream reports a session started outside this daemon's dispatch
    /// path, so it is never counted as orchestrator-originated.
    pub fn session_origin(&self) -> SessionOrigin {
        match self {
            TurnOrigin::Frame { .. } => SessionOrigin::Orchestrator,
            TurnOrigin::Envelope { .. } | TurnOrigin::Operator => SessionOrigin::User,
        }
    }

    /// The driver this origin belongs to.
    pub fn driver(&self) -> SessionDriver {
        match self {
            // An operator turn is typed straight into a live session, which is
            // the same path a task frame drives.
            TurnOrigin::Frame { .. } | TurnOrigin::Operator => SessionDriver::Task,
            TurnOrigin::Envelope { .. } => SessionDriver::Envelope,
        }
    }
}

/// One normalized turn to run against a session.
#[derive(Debug, Clone, PartialEq)]
pub struct TurnRequest {
    /// Which conversation/provider this turn belongs to.
    pub key: SessionKey,
    /// The resolved lifetime class for this turn.
    pub class: SessionClass,
    /// The prompt text handed to the harness.
    pub text: String,
    /// Where the turn came from.
    pub origin: TurnOrigin,
    /// A per-turn model override, when the sender asked for one.
    pub model: Option<String>,
}

/// The terminal result of one turn.
#[derive(Debug, Clone, PartialEq)]
pub struct TurnOutcome {
    /// The agent's final answer text.
    pub reply: String,
    /// Whether the turn was interrupted rather than completing on its own.
    ///
    /// An interrupted turn still ends the **turn**, never the session: an
    /// unbound session survives its own abort and accepts the next turn.
    pub aborted: bool,
    /// Whether the harness reported the turn as an error.
    pub is_error: bool,
    /// The harness's own session id, when it announced one on this turn.
    pub harness_session_id: Option<String>,
}
