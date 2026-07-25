//! Data types for the `stub_server` module.
#[allow(unused_imports)]
use super::*;
/// How the stub should behave for the test in hand.
#[derive(Clone)]
pub(in super::super) struct StubConfig {
    /// The `protocol` int advertised in the `ready` banner (2 ⇒ mismatch).
    pub(in super::super) protocol: i64,
    /// Whether `hello` is answered `ok:true` (false ⇒ a fatal rejection).
    pub(in super::super) hello_ok: bool,
    /// Close the connection right after an `instruct` (to force a reconnect).
    pub(in super::super) drop_after_instruct: bool,
    /// Answer `instruct` with `ok:false` (to exercise the failed-request path).
    pub(in super::super) instruct_fail: bool,
    /// The `event.event` payloads streamed after an `instruct` receipt.
    pub(in super::super) instruct_events: Vec<Value>,
    /// An optional pause between consecutive `instruct_events` writes, so a
    /// test can force the host to observe (and act on) each event alone rather
    /// than folding the whole batch in one wakeup.
    pub(in super::super) instruct_event_gap: Option<std::time::Duration>,
    /// The `event.event` payloads re-emitted when a `subscribe` carries a
    /// `replay` key (a re-attach rebaseline, serve-protocol §5). Empty ⇒ the
    /// stub replays nothing, matching a serve with no recent history.
    pub(in super::super) replay_events: Vec<Value>,
    /// Raw frames to push right after answering `hello` (e.g. a `call` or a
    /// duplicate `ready`), exercising the host's inbound-frame handling.
    pub(in super::super) after_hello: Vec<Value>,
}
/// A running stub. Drop aborts the accept loop and unlinks the socket.
pub(in super::super) struct StubServer {
    /// The socket path to attach a [`CoreRuntime`](super::CoreRuntime) to.
    pub(in super::super) path: PathBuf,
    /// How many connections have been accepted (grows across reconnects).
    pub(super) accepts: Arc<AtomicUsize>,
    /// Every `(op, params)` received, in arrival order.
    pub(super) received: Arc<Mutex<Vec<(String, Value)>>>,
    /// The accept-loop task.
    pub(super) accept_task: JoinHandle<()>,
}
