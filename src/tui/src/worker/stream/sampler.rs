//! The sampler: what turns a live emulator into a frame stream.
//!
//! [`SessionStream`] is the whole decision, and it is pure — it takes a snapshot
//! and returns the frame to send, if any. The tokio task that reads the
//! emulator on a timer and hands frames to the transport is a thin shell around
//! it ([`spawn_session_stream`]), so the interesting behaviour is testable
//! without a pty, a runtime, or a network.
//!
//! Sampling is deliberately driven by a **timer, not by pty output**. A harness
//! repainting a spinner at 60 Hz would otherwise emit sixty frames a second,
//! each one an HTTPS request and a ratchet advance on the lock that also
//! serialises task dispatch. Sampling makes wire cost a function of the
//! subscribed rate alone, however much the harness is churning — which is the
//! property that makes this affordable over a mailbox at all.

use std::sync::Arc;
use std::time::Duration;

use medulla::daemon::SendFn;
use medulla::tinyplace::{
    build_frame, encode_screen_message, FrameDecision, ScreenFrame, ScreenGrid, ScreenMessage,
};

use super::super::pty::{PtyManager, ScreenSnapshot};
use super::convert::wire_grid;

/// The slowest rate a subscription may request.
const MIN_FPS: u8 = 1;
/// The fastest rate a subscription may request.
///
/// Every frame costs a send, a ratchet advance and an acknowledge on the
/// viewer's side, so this is a ceiling on how much of the shared transport one
/// watcher may consume — not a rendering limit.
const MAX_FPS: u8 = 10;

/// How often to sample for a requested rate, clamped to `[MIN_FPS, MAX_FPS]`.
pub fn sample_interval(max_fps: u8) -> Duration {
    let fps = max_fps.clamp(MIN_FPS, MAX_FPS);
    Duration::from_millis(1_000 / u64::from(fps))
}

/// One session's outbound stream state.
///
/// Holds the last screen it put on the wire, so the next frame can be a diff
/// against it. A stream begins owing a full frame, and owes another whenever the
/// viewer says it has lost track.
pub struct SessionStream {
    task_id: String,
    /// Sequence of the last frame actually sent. Frames the sampler skips do not
    /// advance it, so the chain the viewer follows has no gaps in it.
    seq: i64,
    /// The screen the viewer is believed to hold.
    last_sent: Option<ScreenGrid>,
    /// Whether the next frame must be a full one.
    owes_full: bool,
}

impl SessionStream {
    /// Start a stream for `session_id`. The first frame it produces is full.
    pub fn new(task_id: impl Into<String>) -> Self {
        SessionStream {
            task_id: task_id.into(),
            seq: 0,
            last_sent: None,
            owes_full: true,
        }
    }

    /// The sequence of the last frame sent.
    pub fn seq(&self) -> i64 {
        self.seq
    }

    /// Note that the viewer has lost track: the next frame will be full.
    ///
    /// Called for `subscribe { resync: true }`, which is both how a stream
    /// starts and how a viewer recovers from a gap.
    pub fn request_resync(&mut self) {
        self.owes_full = true;
    }

    /// Decide what to send for `snapshot`, advancing the stream if anything goes
    /// out.
    ///
    /// Returns `None` when the screen is unchanged — the common case on an idle
    /// session, and the reason a watched-but-quiet worker costs nothing.
    pub fn tick(&mut self, snapshot: &ScreenSnapshot) -> Option<ScreenFrame> {
        let grid = wire_grid(snapshot);
        // A resync throws away what the viewer was believed to hold, which makes
        // `build_frame` produce a full frame from a clean base.
        let previous = if self.owes_full {
            None
        } else {
            self.last_sent.as_ref()
        };
        let base_seq = if self.owes_full { 0 } else { self.seq };

        match build_frame(previous, &grid, &self.task_id, self.seq + 1, base_seq) {
            FrameDecision::Unchanged => None,
            FrameDecision::Send(frame) => {
                self.seq += 1;
                self.last_sent = Some(grid);
                self.owes_full = false;
                Some(frame)
            }
        }
    }
}

/// Answers "is the task this stream was opened for still running?".
///
/// The stream needs this because a session outlives the task that ran in it —
/// that is what makes an interactive worker interactive — so "the session is
/// still there" is not the same question.
pub type LiveCheck = Arc<dyn Fn() -> bool + Send + Sync>;

/// Everything one subscription needs to know about what it is streaming.
///
/// Grouped rather than passed loose because the parts are meaningless apart: a
/// task, the session it is running in, who asked for it, how fast, and how to
/// tell when it is over.
pub struct StreamSpec {
    /// The task the frames are addressed by.
    pub task_id: String,
    /// The session the frames are sampled from.
    pub session_id: String,
    /// The peer the frames are sent to.
    pub subscriber: String,
    /// Frames per second asked for, before clamping.
    pub max_fps: u8,
    /// Answers whether the task is still running.
    pub is_live: LiveCheck,
}

/// Drive one subscription: sample the session on a timer and send what changes.
///
/// Ends when its task ends, or when the session disappears. Both matter, and
/// the first is the one that is easy to miss: a session is handed on to the
/// next task once its current one settles, so a stream that only watched the
/// session would carry on sampling — and keep labelling another task's screen,
/// possibly another peer's, with the task id this stream was opened for.
///
/// Dropping the returned handle aborts it, which is how `unsubscribe` is served.
pub fn spawn_session_stream(
    sessions: PtyManager,
    spec: StreamSpec,
    send: SendFn,
) -> tokio::task::JoinHandle<()> {
    let StreamSpec {
        task_id,
        session_id,
        subscriber,
        max_fps,
        is_live,
    } = spec;
    let interval = sample_interval(max_fps);
    tokio::spawn(async move {
        // Frames are addressed by task — what the subscriber named and holds —
        // while sampling reads the session the task is running in.
        let mut stream = SessionStream::new(task_id);
        loop {
            // Checked before the session, because a settled task is the common
            // ending and the session it ran in is usually still very much alive.
            if !is_live() {
                return; // the task is over; the screen is no longer its screen
            }
            // Read the emulator, never the pty: `screen_rows` hands back an
            // owned copy so the sampler never holds the parser lock across the
            // send, which would stall the reader thread feeding it.
            let Some(snapshot) = sessions.screen_rows(&session_id) else {
                return; // the session is gone; so is the subscription
            };
            if let Some(frame) = stream.tick(&snapshot) {
                let body = encode_screen_message(&ScreenMessage::Frame(frame));
                send(subscriber.clone(), body).await;
            }
            tokio::time::sleep(interval).await;
        }
    })
}

/// The live subscriptions this worker is serving, keyed by task.
///
/// One viewer per task: a second `subscribe` replaces the first rather than
/// fanning out, since two watchers would double the transport cost for no new
/// information.
#[derive(Default)]
pub struct StreamRegistry {
    /// Keyed by task, holding the subscriber it was opened for alongside the
    /// running sampler. The subscriber is kept so a stop can be authorized
    /// against *who asked for the stream* rather than against whether the task
    /// is still running — the latter refuses the one legitimate case that
    /// matters, a viewer letting go of a task that has just finished.
    streams: std::collections::HashMap<String, (String, tokio::task::JoinHandle<()>)>,
}

impl StreamRegistry {
    /// An empty registry.
    pub fn new() -> Self {
        StreamRegistry::default()
    }

    /// Start (or restart) the stream for `session_id`.
    ///
    /// Restarting is how a resync is served: the replacement stream begins
    /// owing a full frame, which is exactly what the viewer asked for.
    pub fn subscribe(&mut self, sessions: &PtyManager, spec: StreamSpec, send: SendFn) {
        let (task_id, subscriber) = (spec.task_id.clone(), spec.subscriber.clone());
        self.unsubscribe(&task_id);
        let handle = spawn_session_stream(sessions.clone(), spec, send);
        self.streams.insert(task_id, (subscriber, handle));
    }

    /// Stop streaming `task_id`, if it was being streamed.
    pub fn unsubscribe(&mut self, task_id: &str) {
        if let Some((_, handle)) = self.streams.remove(task_id) {
            handle.abort();
        }
    }

    /// Stop streaming `task_id`, but only for the peer it is being streamed to.
    ///
    /// Returns whether a stream was stopped. This is the authorization the
    /// protocol actually needs: without it any peer could cancel another's
    /// stream by naming its task id, and checking the task is still running
    /// instead — as this once did — leaves a finished task's stream impossible
    /// to stop by anyone.
    pub fn unsubscribe_for(&mut self, subscriber: &str, task_id: &str) -> bool {
        let matches = self
            .streams
            .get(task_id)
            .is_some_and(|(owner, _)| owner == subscriber);
        if matches {
            self.unsubscribe(task_id);
        }
        matches
    }

    /// How many sessions are being streamed.
    pub fn len(&self) -> usize {
        self.streams.len()
    }

    /// Whether `task_id` already has a live stream.
    pub fn contains(&self, task_id: &str) -> bool {
        self.streams
            .get(task_id)
            .is_some_and(|(_, handle)| !handle.is_finished())
    }

    /// Whether nothing is being streamed.
    pub fn is_empty(&self) -> bool {
        self.streams.is_empty()
    }

    /// Drop every subscription — used on shutdown.
    pub fn clear(&mut self) {
        for (_, (_, handle)) in self.streams.drain() {
            handle.abort();
        }
    }
}

/// Reap subscriptions whose task has ended, so a registry driven by a long-lived
/// worker does not accumulate handles for sessions that exited.
impl StreamRegistry {
    /// Forget every stream whose task has finished.
    pub fn prune(&mut self) {
        self.streams.retain(|_, (_, handle)| !handle.is_finished());
    }
}

/// Build a [`SendFn`] over a closure, matching the shape the daemon runtime
/// already takes so a caller can hand both the same transport.
pub fn send_fn<F, Fut>(f: F) -> SendFn
where
    F: Fn(String, String) -> Fut + Send + Sync + 'static,
    Fut: std::future::Future<Output = ()> + Send + 'static,
{
    Arc::new(move |to, body| Box::pin(f(to, body)) as _)
}
