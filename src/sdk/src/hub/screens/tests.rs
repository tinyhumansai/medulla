//! Unit tests for the hub's screen cache.

use super::{ScreenStore, WatchedScreen};
use crate::protocol::{build_frame, ApplyOutcome, FrameDecision, ScreenGrid, ScreenRun};

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
    task: &str,
    seq: i64,
    base: i64,
) -> crate::protocol::ScreenFrame {
    match build_frame(previous, next, task, seq, base) {
        FrameDecision::Send(frame) => frame,
        FrameDecision::Unchanged => panic!("expected a frame"),
    }
}

#[test]
fn a_full_frame_establishes_a_screen() {
    let store = ScreenStore::new();
    let screen = grid("hello");
    let outcome = store.apply("workerA", &frame(None, &screen, "w_1", 1, 0), 100);

    assert_eq!(outcome, ApplyOutcome::Applied);
    let held = store.get("workerA", "w_1").expect("a screen");
    assert_eq!(held.grid, screen);
    assert_eq!(held.seq, 1);
    assert_eq!(held.updated_at, 100);
}

#[test]
fn a_delta_advances_the_held_screen() {
    let store = ScreenStore::new();
    let first = grid("hello");
    let second = grid("world");
    store.apply("workerA", &frame(None, &first, "w_1", 1, 0), 100);
    let outcome = store.apply("workerA", &frame(Some(&first), &second, "w_1", 2, 1), 200);

    assert_eq!(outcome, ApplyOutcome::Applied);
    let held = store.get("workerA", "w_1").expect("a screen");
    assert_eq!(held.grid, second);
    assert_eq!(held.seq, 2);
    assert_eq!(held.updated_at, 200);
}

#[test]
fn a_gap_asks_to_resync_and_leaves_the_held_screen_alone() {
    // What is on display must stay a screen the worker really showed, never a
    // partly patched one.
    let store = ScreenStore::new();
    let first = grid("hello");
    let second = grid("world");
    store.apply("workerA", &frame(None, &first, "w_1", 1, 0), 100);

    // A delta claiming to follow seq 7 when the store holds seq 1.
    let mut orphan = frame(Some(&first), &second, "w_1", 8, 7);
    orphan.base_seq = 7;
    let outcome = store.apply("workerA", &orphan, 200);

    assert_eq!(outcome, ApplyOutcome::NeedsResync);
    let held = store.get("workerA", "w_1").expect("the screen survives");
    assert_eq!(held.grid, first, "the stale screen is kept intact");
    assert_eq!(held.seq, 1);
    assert_eq!(held.updated_at, 100, "a rejected frame is not an update");
}

#[test]
fn a_first_delta_with_nothing_held_asks_to_resync() {
    let store = ScreenStore::new();
    let first = grid("hello");
    let second = grid("world");
    let outcome = store.apply("workerA", &frame(Some(&first), &second, "w_1", 2, 1), 100);
    assert_eq!(outcome, ApplyOutcome::NeedsResync);
    assert!(store.is_empty(), "nothing should be recorded");
}

#[test]
fn screens_are_keyed_by_worker_and_session() {
    // Two workers can each run a session called `w_1`; they must not collide.
    let store = ScreenStore::new();
    let a = grid("from A");
    let b = grid("from B");
    store.apply("workerA", &frame(None, &a, "w_1", 1, 0), 100);
    store.apply("workerB", &frame(None, &b, "w_1", 1, 0), 100);

    assert_eq!(store.len(), 2);
    assert_eq!(store.get("workerA", "w_1").unwrap().grid, a);
    assert_eq!(store.get("workerB", "w_1").unwrap().grid, b);
}

#[test]
fn for_worker_returns_only_that_workers_screens_newest_first() {
    let store = ScreenStore::new();
    store.apply("workerA", &frame(None, &grid("one"), "w_1", 1, 0), 100);
    store.apply("workerA", &frame(None, &grid("two"), "w_2", 1, 0), 300);
    store.apply("workerB", &frame(None, &grid("other"), "w_1", 1, 0), 200);

    let mine = store.for_worker("workerA");
    assert_eq!(mine.len(), 2);
    assert_eq!(mine[0].task_id, "w_2", "most recently updated first");
    assert_eq!(mine[1].task_id, "w_1");
}

#[test]
fn the_store_is_bounded_and_drops_the_stalest() {
    // A hub watching a fan-out must not hold a framebuffer per worker forever.
    let store = ScreenStore::new();
    for n in 0..12 {
        store.apply(
            "workerA",
            &frame(None, &grid("x"), &format!("w_{n}"), 1, 0),
            // Ascending, so `w_0` is the stalest throughout.
            100 + n as i64,
        );
    }
    assert_eq!(store.len(), 8, "capped at CAPACITY");
    assert!(store.get("workerA", "w_0").is_none(), "stalest evicted");
    assert!(store.get("workerA", "w_11").is_some(), "newest kept");
}

#[test]
fn forgetting_a_screen_removes_only_that_one() {
    let store = ScreenStore::new();
    store.apply("workerA", &frame(None, &grid("one"), "w_1", 1, 0), 100);
    store.apply("workerA", &frame(None, &grid("two"), "w_2", 1, 0), 100);

    store.forget("workerA", "w_1");
    assert!(store.get("workerA", "w_1").is_none());
    assert!(store.get("workerA", "w_2").is_some());
}

#[test]
fn an_empty_store_reports_empty() {
    let store = ScreenStore::new();
    assert!(store.is_empty());
    assert!(store.snapshot().is_empty());
    assert!(store.for_worker("nobody").is_empty());
    assert_eq!(store.get("nobody", "w_1"), None::<WatchedScreen>);
}

#[test]
fn intent_is_tracked_separately_from_what_has_arrived() {
    let store = ScreenStore::new();

    // Watching starts before anything has been received: the subscribe and the
    // first frame are a relay round-trip apart, and frames arriving in that
    // window are wanted.
    assert!(!store.is_watching("workerA", "w_1"));
    store.arm("workerA", "w_1");
    assert!(store.is_watching("workerA", "w_1"));
    assert!(store.get("workerA", "w_1").is_none(), "nothing held yet");

    let screen = grid("hello");
    store.apply("workerA", &frame(None, &screen, "w_1", 1, 0), 100);
    assert!(store.get("workerA", "w_1").is_some());

    // Disarming stops the watch but keeps the screen: looking away must not
    // throw away a frame already paid for, or looking back would show a blank
    // pane until the relay round trip completes.
    store.disarm("workerA", "w_1");
    assert!(!store.is_watching("workerA", "w_1"));
    assert!(store.get("workerA", "w_1").is_some(), "the screen is kept");

    // Forgetting drops both — for a worker that has gone away.
    store.forget("workerA", "w_1");
    assert!(store.get("workerA", "w_1").is_none());
}

#[test]
fn watching_one_task_says_nothing_about_another() {
    let store = ScreenStore::new();
    store.arm("workerA", "w_1");

    assert!(!store.is_watching("workerA", "w_2"), "a different task");
    assert!(!store.is_watching("workerB", "w_1"), "a different worker");
}
