//! Unit tests for the pump's screen routing.
//!
//! These cover what the pump *narrates*, not just what it stores. A screen that
//! never appears has several indistinguishable causes — the worker was never
//! asked, the worker refused, frames arrive and the viewer is looking elsewhere
//! — and the log line is the only thing that separates them from the hub's side.

use std::sync::{Arc, Mutex};

use async_trait::async_trait;

use super::route_screen;
use crate::hub::relay::Relay;
use crate::hub::ScreenStore;
use crate::protocol::{
    build_frame, parse_screen_message, FrameDecision, ScreenGrid, ScreenMessage, ScreenRun,
};

/// A relay that records what the pump sent and answers nothing.
#[derive(Default)]
struct Recorder {
    sent: Mutex<Vec<String>>,
}

#[async_trait]
impl Relay for Recorder {
    async fn send(&self, _to: &str, body: &str) -> Result<(), String> {
        self.sent.lock().unwrap().push(body.to_string());
        Ok(())
    }

    async fn drain_inbox(&self, _limit: i64) -> Vec<crate::bridge::InboundMessage> {
        Vec::new()
    }

    async fn reset_session(&self, _peer: &str) {}
}

/// Collects the lines the pump logged.
fn recording_log() -> (crate::hub::types::HubLog, Arc<Mutex<Vec<String>>>) {
    let lines = Arc::new(Mutex::new(Vec::new()));
    let sink = lines.clone();
    let log: crate::hub::types::HubLog =
        Arc::new(move |line: &str| sink.lock().unwrap().push(line.to_string()));
    (log, lines)
}

/// A one-row grid holding `text`.
fn grid(text: &str) -> ScreenGrid {
    ScreenGrid {
        cols: text.len() as u16,
        rows: 1,
        lines: vec![vec![ScreenRun::plain(text)]],
        cursor: (0, 0),
        hide_cursor: false,
    }
}

/// The frame carrying `next` to a viewer holding `previous`.
fn frame(
    previous: Option<&ScreenGrid>,
    next: &ScreenGrid,
    seq: i64,
    base: i64,
) -> crate::protocol::ScreenFrame {
    match build_frame(previous, next, "w_1", seq, base) {
        FrameDecision::Send(frame) => frame,
        FrameDecision::Unchanged => panic!("expected a frame"),
    }
}

#[tokio::test]
async fn the_first_frame_of_a_stream_is_announced_once() {
    let relay = Recorder::default();
    let screens = ScreenStore::new();
    screens.arm("workerA", "w_1");
    let (log, lines) = recording_log();
    let log = Some(log);
    let first = grid("hello");
    let second = grid("world");

    route_screen(
        &relay,
        &screens,
        "workerA",
        ScreenMessage::Frame(frame(None, &first, 1, 0)),
        &log,
    )
    .await;
    route_screen(
        &relay,
        &screens,
        "workerA",
        ScreenMessage::Frame(frame(Some(&first), &second, 2, 1)),
        &log,
    )
    .await;

    // One line, not two: at a frame a second, per-frame narration would bury
    // every other line in the hub log within a minute.
    let lines = lines.lock().unwrap();
    assert_eq!(lines.len(), 1, "expected one line, got {lines:?}");
    assert!(
        lines[0].contains("streaming at 5x1") && lines[0].contains("w_1"),
        "{}",
        lines[0]
    );
    // Narration only. A healthy stream asks the worker for nothing.
    assert!(relay.sent.lock().unwrap().is_empty());
}

#[tokio::test]
async fn a_frame_the_hub_cannot_apply_asks_for_a_resync_and_says_so() {
    let relay = Recorder::default();
    let screens = ScreenStore::new();
    screens.arm("workerA", "w_1");
    let (log, lines) = recording_log();
    let log = Some(log);
    let first = grid("hello");
    let second = grid("world");

    // A delta whose base the hub never held: the frame establishing seq 1 was
    // lost, so seq 2 has nothing to apply against.
    route_screen(
        &relay,
        &screens,
        "workerA",
        ScreenMessage::Frame(frame(Some(&first), &second, 2, 1)),
        &log,
    )
    .await;

    let lines = lines.lock().unwrap();
    assert_eq!(lines.len(), 1, "expected one line, got {lines:?}");
    assert!(lines[0].contains("out of step at seq 2"), "{}", lines[0]);

    // And the hub repairs itself rather than sitting on a screen that will never
    // advance again.
    let sent = relay.sent.lock().unwrap();
    assert_eq!(sent.len(), 1);
    match parse_screen_message(&sent[0]).expect("a screen message") {
        ScreenMessage::Subscribe {
            task_id, resync, ..
        } => {
            assert_eq!(task_id, "w_1");
            assert!(resync, "a repair subscribe must ask for a full frame");
        }
        other => panic!("expected a subscribe, got {other:?}"),
    }
}

#[tokio::test]
async fn the_hub_ignores_screen_messages_only_a_viewer_would_send() {
    let relay = Recorder::default();
    let screens = ScreenStore::new();
    let (log, lines) = recording_log();

    // The hub is the viewer. A subscribe arriving here is a peer with the
    // protocol backwards, and answering it would be the hub streaming its own
    // screen to a worker.
    route_screen(
        &relay,
        &screens,
        "workerA",
        ScreenMessage::Subscribe {
            task_id: "w_1".to_string(),
            max_fps: 1,
            resync: false,
        },
        &Some(log),
    )
    .await;

    assert!(lines.lock().unwrap().is_empty());
    assert!(relay.sent.lock().unwrap().is_empty());
    assert!(screens.get("workerA", "w_1").is_none());
}

#[tokio::test]
async fn a_frame_for_a_task_nobody_watches_is_still_kept() {
    let relay = Recorder::default();
    let screens = ScreenStore::new();
    let (log, _lines) = recording_log();
    let screen = grid("hello");

    // Never armed. The frame crossed the relay and was decrypted before this
    // function saw it, so discarding it would save nothing and throw away the
    // freshest screen this hub will ever hold for the task — which is exactly
    // what the operator wants drawn the moment they look back at it.
    route_screen(
        &relay,
        &screens,
        "workerA",
        ScreenMessage::Frame(frame(None, &screen, 1, 0)),
        &Some(log),
    )
    .await;

    assert_eq!(
        screens.get("workerA", "w_1").expect("a screen").grid,
        screen
    );
    // But nothing is asked of the worker: a resync request is a subscribe, and
    // sending one here is what used to restart a stream nobody was watching.
    assert!(relay.sent.lock().unwrap().is_empty());
}

#[tokio::test]
async fn a_frame_still_in_flight_when_a_watch_ends_does_not_restart_it() {
    let relay = Recorder::default();
    let screens = ScreenStore::new();
    screens.arm("workerA", "w_1");
    let (log, _lines) = recording_log();
    let log = Some(log);
    let first = grid("hello");
    let second = grid("world");

    route_screen(
        &relay,
        &screens,
        "workerA",
        ScreenMessage::Frame(frame(None, &first, 1, 0)),
        &log,
    )
    .await;

    // The operator looks away. The relay is not instant, so the worker has
    // already sent the next frame by the time its unsubscribe lands.
    screens.disarm("workerA", "w_1");
    relay.sent.lock().unwrap().clear();

    route_screen(
        &relay,
        &screens,
        "workerA",
        ScreenMessage::Frame(frame(Some(&first), &second, 2, 1)),
        &log,
    )
    .await;

    // The regression: that late frame was answered with `Subscribe { resync }`,
    // which restarted the stream that had just been stopped, permanently.
    assert!(
        relay.sent.lock().unwrap().is_empty(),
        "a late frame must never be answered with a subscribe: {:?}",
        relay.sent.lock().unwrap()
    );
    // And because the screen was kept rather than dropped, the late frame still
    // applies — so looking back shows the newer screen, not the older one.
    assert_eq!(
        screens.get("workerA", "w_1").expect("a screen").grid,
        second
    );
}

#[tokio::test]
async fn an_unwatched_frame_that_cannot_be_applied_asks_for_nothing() {
    let relay = Recorder::default();
    let screens = ScreenStore::new();
    let (log, lines) = recording_log();
    let first = grid("hello");
    let second = grid("world");

    // A delta with no base held, for a task nobody is watching: the frame is
    // unusable *and* unwanted. The first alone would justify a resync; the
    // second is what must win.
    route_screen(
        &relay,
        &screens,
        "workerA",
        ScreenMessage::Frame(frame(Some(&first), &second, 2, 1)),
        &Some(log),
    )
    .await;

    assert!(relay.sent.lock().unwrap().is_empty());
    assert!(lines.lock().unwrap().is_empty());
    assert!(screens.get("workerA", "w_1").is_none());
}
