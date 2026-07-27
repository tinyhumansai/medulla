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

/// Drive one subscription: sample the session on a timer and send what changes.
///
/// Ends when the session disappears — the harness exited, or the manager forgot
/// it — so a subscription cannot outlive the thing it is watching. Dropping the
/// returned handle aborts it, which is how `unsubscribe` is served.
pub fn spawn_session_stream(
    sessions: PtyManager,
    task_id: String,
    session_id: String,
    subscriber: String,
    max_fps: u8,
    send: SendFn,
) -> tokio::task::JoinHandle<()> {
    let interval = sample_interval(max_fps);
    tokio::spawn(async move {
        // Frames are addressed by task — what the subscriber named and holds —
        // while sampling reads the session the task is running in.
        let mut stream = SessionStream::new(task_id);
        loop {
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
    streams: std::collections::HashMap<String, tokio::task::JoinHandle<()>>,
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
    pub fn subscribe(
        &mut self,
        sessions: &PtyManager,
        task_id: &str,
        session_id: &str,
        subscriber: &str,
        max_fps: u8,
        send: SendFn,
    ) {
        self.unsubscribe(task_id);
        let handle = spawn_session_stream(
            sessions.clone(),
            task_id.to_string(),
            session_id.to_string(),
            subscriber.to_string(),
            max_fps,
            send,
        );
        self.streams.insert(task_id.to_string(), handle);
    }

    /// Stop streaming `task_id`, if it was being streamed.
    pub fn unsubscribe(&mut self, task_id: &str) {
        if let Some(handle) = self.streams.remove(task_id) {
            handle.abort();
        }
    }

    /// How many sessions are being streamed.
    pub fn len(&self) -> usize {
        self.streams.len()
    }

    /// Whether `task_id` already has a live stream.
    pub fn contains(&self, task_id: &str) -> bool {
        self.streams
            .get(task_id)
            .is_some_and(|handle| !handle.is_finished())
    }

    /// Whether nothing is being streamed.
    pub fn is_empty(&self) -> bool {
        self.streams.is_empty()
    }

    /// Drop every subscription — used on shutdown.
    pub fn clear(&mut self) {
        for (_, handle) in self.streams.drain() {
            handle.abort();
        }
    }
}

/// Reap subscriptions whose task has ended, so a registry driven by a long-lived
/// worker does not accumulate handles for sessions that exited.
impl StreamRegistry {
    /// Forget every stream whose task has finished.
    pub fn prune(&mut self) {
        self.streams.retain(|_, handle| !handle.is_finished());
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
