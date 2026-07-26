//! Data types for the `harness` module.

#[allow(unused_imports)]
use super::*;

/// How the fake worker responds to a dispatched task.
pub(in crate::hub::tests) enum Mode {
    Reply(String),
    Error(String),
    Silent,
    /// Acks (a sign of life) and streams a status, but never sends a terminal
    /// frame — exercises the "peer alive, then goes silent" path the no-progress
    /// watchdog must reap.
    AckOnly,
    /// Silent until the sender has reset the session (simulating a restarted peer
    /// whose first `CIPHERTEXT` is undecryptable), then replies.
    RecoverAfterReset(String),
    /// Streams `statuses` progress frames, one per drain, then replies. Models a
    /// worker that is plainly working but runs longer than the idle window — the
    /// case a wall-clock deadline (or the old ceiling) kills and a frame-resetting
    /// idle watchdog does not.
    Chatty {
        statuses: u32,
        reply: String,
    },
    /// Queues an undecodable message ahead of the normal ack→reply, exercising the
    /// pump's skip-and-continue path for a frame it cannot parse (a stray DM or a
    /// corrupt payload landing in the shared inbox).
    GarbageThenReply(String),
    /// Answers a lightweight capacity probe without starting a task.
    SystemInfo(WorkerSystemInfo),
    /// Answers a capacity probe only after the sender repairs the session.
    SystemInfoAfterReset(WorkerSystemInfo),
    /// Answers a capacity probe with malformed JSON.
    InvalidSystemInfo,
    /// Answers a capability probe with the given [`AgentCapabilities`] (its
    /// budgets/readiness serialized into a `capabilities_result` frame).
    Capabilities(AgentCapabilities),
    /// A third peer answers first, under the dispatch's own correlation id, and
    /// the real worker replies after it. Models the shared inbox: every contact
    /// of this identity can post to it, and correlation ids are not secrets.
    ImpostorThenReply {
        /// The address the injected frames claim to come from.
        impostor: String,
        /// What the impostor claims the task returned.
        stolen: String,
        /// What the real worker actually returns.
        reply: String,
    },
}

pub(in crate::hub::tests) struct FakeWorker {
    /// The kind of every frame the runner sent us, in order.
    pub(super) sent: Mutex<Vec<String>>,
    pub(super) inbox: Mutex<VecDeque<InboundMessage>>,
    pub(super) mode: Mode,
    /// How many times the sender has reset the session with us.
    pub(in crate::hub::tests) resets: AtomicU32,
    /// When true, every `send` fails — exercises the transport-error path.
    pub(super) fail_send: bool,
    /// `contact_accepted` returns false until it has been polled this many times,
    /// simulating a peer whose auto-accepter settles a few polls later.
    pub(super) accept_after: u32,
    /// How many times `contact_accepted` has been polled.
    pub(in crate::hub::tests) contact_checks: AtomicU32,
}
