//! Unit tests for the worker's half: the emulator-to-wire conversion and the
//! sampler's frame decision.
//!
//! Both are pure, so none of this needs a pty, a runtime or a network. The
//! end-to-end property — a viewer folding what a sampler emits ends up holding
//! the emulator's screen — is asserted here too, since it is the only check that
//! covers the conversion and the diff *together*.

use medulla::tinyplace::{
    apply_frame, ApplyOutcome, Color, ScreenRun, ScreenView, ATTR_BOLD, ATTR_INVERSE,
    ATTR_UNDERLINE,
};

use super::super::pty::{ScreenCell, ScreenSnapshot};
use super::*;

/// A cell with text and no styling.
fn cell(text: &str) -> ScreenCell {
    ScreenCell {
        text: text.into(),
        ..ScreenCell::default()
    }
}

/// A snapshot from rows of text, one cell per character.
fn snapshot(rows: &[&str]) -> ScreenSnapshot {
    ScreenSnapshot {
        cells: rows
            .iter()
            .map(|row| row.chars().map(|c| cell(&c.to_string())).collect())
            .collect(),
        cursor: (0, 0),
        hide_cursor: false,
    }
}

// --- conversion ------------------------------------------------------------

#[test]
fn colours_map_across_without_being_resolved() {
    // Default must stay Default: the viewer inherits its own palette for
    // unstyled text, exactly as the local renderer does.
    assert_eq!(wire_color(vt100::Color::Default), Color::Default);
    assert_eq!(wire_color(vt100::Color::Idx(4)), Color::Idx(4));
    assert_eq!(wire_color(vt100::Color::Rgb(1, 2, 3)), Color::Rgb(1, 2, 3));
}

#[test]
fn attributes_become_flags() {
    let styled = ScreenCell {
        text: "x".into(),
        bold: true,
        underline: true,
        ..ScreenCell::default()
    };
    let style = wire_style(&styled);
    assert!(style.has(ATTR_BOLD));
    assert!(style.has(ATTR_UNDERLINE));
    assert!(!style.has(ATTR_INVERSE));
}

#[test]
fn inverse_is_carried_as_a_flag_not_baked_into_the_colours() {
    // The local renderer swaps fg/bg at paint time because terminals disagree
    // about REVERSED. Baking that into the wire would leave the viewer unable to
    // tell an inverted cell from a deliberately colour-swapped one.
    let inverted = ScreenCell {
        text: "x".into(),
        fg: vt100::Color::Idx(1),
        bg: vt100::Color::Idx(7),
        inverse: true,
        ..ScreenCell::default()
    };
    let style = wire_style(&inverted);
    assert_eq!(style.fg, Color::Idx(1), "colours must not be pre-swapped");
    assert_eq!(style.bg, Color::Idx(7));
    assert!(style.has(ATTR_INVERSE));
}

#[test]
fn a_grid_takes_its_size_from_the_snapshot() {
    let grid = wire_grid(&snapshot(&["abcd", "efgh", "ijkl"]));
    assert_eq!(grid.rows, 3);
    assert_eq!(grid.cols, 4);
    assert_eq!(grid.lines.len(), 3);
    assert_eq!(grid.lines[0], vec![ScreenRun::plain("abcd")]);
}

#[test]
fn rows_are_coalesced_and_trailing_blanks_dropped() {
    // A 120-column screen is mostly blank; carrying it cell by cell would
    // dominate every frame.
    let grid = wire_grid(&snapshot(&["hi        "]));
    assert_eq!(grid.lines[0], vec![ScreenRun::plain("hi")]);
    assert_eq!(grid.cols, 10, "the row is still ten cells wide");
}

#[test]
fn an_empty_snapshot_converts_without_panicking() {
    let grid = wire_grid(&ScreenSnapshot {
        cells: Vec::new(),
        cursor: (0, 0),
        hide_cursor: false,
    });
    assert_eq!(grid.rows, 0);
    assert_eq!(grid.cols, 0);
}

// --- the sampler -----------------------------------------------------------

#[test]
fn the_first_tick_is_a_full_frame() {
    let mut stream = SessionStream::new("w_1");
    let frame = stream.tick(&snapshot(&["a", "b"])).expect("a first frame");
    assert!(frame.full);
    assert_eq!(frame.seq, 1);
    assert_eq!(stream.seq(), 1);
}

#[test]
fn an_unchanged_screen_produces_nothing_and_does_not_advance_the_seq() {
    // The gap-free chain matters: a skipped frame that still burned a sequence
    // number would make the viewer's next base_seq check fail for no reason.
    let mut stream = SessionStream::new("w_1");
    let screen = snapshot(&["steady"]);
    assert!(stream.tick(&screen).is_some());
    assert_eq!(stream.seq(), 1);
    assert!(stream.tick(&screen).is_none());
    assert!(stream.tick(&screen).is_none());
    assert_eq!(stream.seq(), 1, "skipped frames must not advance the chain");
}

#[test]
fn a_change_after_a_quiet_stretch_still_chains_from_the_last_sent_frame() {
    let mut stream = SessionStream::new("w_1");
    stream.tick(&snapshot(&["one", "two"]));
    stream.tick(&snapshot(&["one", "two"]));
    let frame = stream
        .tick(&snapshot(&["one", "CHANGED"]))
        .expect("a change is a frame");
    assert!(!frame.full);
    assert_eq!(frame.seq, 2);
    assert_eq!(
        frame.base_seq, 1,
        "chains from the last frame actually sent"
    );
    assert_eq!(frame.rows_changed.len(), 1);
    assert_eq!(frame.rows_changed[0].y, 1);
}

#[test]
fn a_resync_request_forces_the_next_frame_full() {
    let mut stream = SessionStream::new("w_1");
    stream.tick(&snapshot(&["a", "b"]));
    let delta = stream.tick(&snapshot(&["a", "c"])).expect("a delta");
    assert!(!delta.full);

    stream.request_resync();
    let full = stream
        .tick(&snapshot(&["a", "c"]))
        .expect("a resync sends even though nothing changed");
    assert!(full.full, "a resync must produce a full frame");
    assert_eq!(full.rows_changed.len(), 2, "the whole screen, not a delta");
}

#[test]
fn a_resize_produces_a_full_frame_by_itself() {
    let mut stream = SessionStream::new("w_1");
    stream.tick(&snapshot(&["abc"]));
    let frame = stream
        .tick(&snapshot(&["abcdef"]))
        .expect("a resize is a change");
    assert!(frame.full, "row indices do not survive a resize");
    assert_eq!(frame.cols, 6);
}

#[test]
fn the_sample_interval_is_clamped_to_a_sane_range() {
    use std::time::Duration;
    assert_eq!(sample_interval(1), Duration::from_millis(1000));
    assert_eq!(sample_interval(4), Duration::from_millis(250));
    assert_eq!(sample_interval(10), Duration::from_millis(100));
    // A zero would divide by zero; an absurd rate would flood the transport.
    assert_eq!(sample_interval(0), Duration::from_millis(1000));
    assert_eq!(sample_interval(255), Duration::from_millis(100));
}

// --- sampler and viewer together -------------------------------------------

#[test]
fn a_viewer_folding_the_samplers_frames_ends_up_holding_the_emulators_screen() {
    // The only check that covers the conversion and the diff together — either
    // one being subtly wrong shows up here and nowhere else.
    let screens = [
        snapshot(&["one   ", "two   ", "three "]),
        snapshot(&["one   ", "TWO!  ", "three "]),
        snapshot(&["one   ", "TWO!  ", "three "]), // idle: nothing sent
        snapshot(&["wider now", "and     ", "taller  "]), // resize
    ];

    let mut stream = SessionStream::new("w_1");
    let mut view: Option<ScreenView> = None;

    for screen in &screens {
        if let Some(frame) = stream.tick(screen) {
            assert_eq!(
                apply_frame(&mut view, &frame),
                ApplyOutcome::Applied,
                "every frame the sampler emits must apply in order"
            );
        }
        assert_eq!(
            &view.as_ref().expect("a view by now").grid,
            &wire_grid(screen),
            "viewer diverged from the emulator"
        );
    }
}

#[test]
fn a_viewer_that_missed_a_frame_recovers_through_a_resync() {
    let mut stream = SessionStream::new("w_1");
    let mut view: Option<ScreenView> = None;

    let first = stream.tick(&snapshot(&["a", "b"])).expect("first");
    apply_frame(&mut view, &first);

    // The frame carrying this change never reaches the viewer.
    let _lost = stream.tick(&snapshot(&["a", "LOST"])).expect("second");

    // So the next delta is unusable: it chains from a state the viewer lacks.
    let orphan = stream.tick(&snapshot(&["a", "AGAIN"])).expect("third");
    assert_eq!(apply_frame(&mut view, &orphan), ApplyOutcome::NeedsResync);

    // The viewer asks to resync; the next frame is full and closes the gap
    // without anything being retransmitted.
    stream.request_resync();
    let recovered = stream.tick(&snapshot(&["a", "AGAIN"])).expect("resync");
    assert_eq!(apply_frame(&mut view, &recovered), ApplyOutcome::Applied);
    assert_eq!(
        view.expect("recovered").grid,
        wire_grid(&snapshot(&["a", "AGAIN"]))
    );
}

#[test]
fn styling_survives_the_whole_round_trip() {
    let mut screen = snapshot(&["ab"]);
    screen.cells[0][0] = ScreenCell {
        text: "a".into(),
        fg: vt100::Color::Idx(4),
        bold: true,
        ..ScreenCell::default()
    };
    screen.cells[0][1] = ScreenCell {
        text: "b".into(),
        bg: vt100::Color::Rgb(9, 9, 9),
        inverse: true,
        ..ScreenCell::default()
    };

    let mut stream = SessionStream::new("w_1");
    let frame = stream.tick(&screen).expect("a frame");
    let mut view = None;
    apply_frame(&mut view, &frame);

    let grid = view.expect("applied").grid;
    assert_eq!(
        grid.lines[0].len(),
        2,
        "different styles stay separate runs"
    );
    assert_eq!(grid.lines[0][0].style.fg, Color::Idx(4));
    assert!(grid.lines[0][0].style.has(ATTR_BOLD));
    assert_eq!(grid.lines[0][1].style.bg, Color::Rgb(9, 9, 9));
    assert!(grid.lines[0][1].style.has(ATTR_INVERSE));
}

// --- the spawned task ------------------------------------------------------

#[tokio::test]
async fn a_subscription_ends_when_its_session_does_not_exist() {
    // A stream must not outlive the thing it is watching, and must not sit
    // spinning against a session id that was never there.
    use std::sync::{Arc, Mutex};
    use std::time::Duration;

    let sent: Arc<Mutex<Vec<(String, String)>>> = Arc::new(Mutex::new(Vec::new()));
    let recorder = sent.clone();
    let send = send_fn(move |to, body| {
        let recorder = recorder.clone();
        async move {
            recorder.lock().unwrap().push((to, body));
        }
    });

    let handle = spawn_session_stream(
        super::super::pty::PtyManager::new(),
        spec("t1", "w_missing", "peer", 10, always_live()),
        send,
    );

    tokio::time::timeout(Duration::from_secs(5), handle)
        .await
        .expect("the task must end rather than spin")
        .expect("and end cleanly");
    assert!(
        sent.lock().unwrap().is_empty(),
        "nothing should be sent for a session that does not exist"
    );
}

#[tokio::test]
async fn unsubscribing_stops_the_stream() {
    let sessions = super::super::pty::PtyManager::new();
    let send = send_fn(|_, _| async {});
    let mut registry = StreamRegistry::new();

    assert!(registry.is_empty());
    registry.subscribe(
        &sessions,
        spec("t1", "w_1", "peer", 1, always_live()),
        send.clone(),
    );
    assert_eq!(registry.len(), 1);

    // A second subscribe replaces rather than fans out: two watchers on one
    // session would double its transport cost for no new information.
    registry.subscribe(&sessions, spec("t1", "w_1", "peer", 1, always_live()), send);
    assert_eq!(registry.len(), 1);

    registry.unsubscribe("t1");
    assert!(registry.is_empty());
}

// --- the router ------------------------------------------------------------

/// A liveness check that always says yes, for the tests that are about
/// something else.
fn always_live() -> super::sampler::LiveCheck {
    std::sync::Arc::new(|| true)
}

/// A [`StreamSpec`] from its parts, so the tests read as call sites rather than
/// struct literals.
fn spec(
    task_id: &str,
    session_id: &str,
    subscriber: &str,
    max_fps: u8,
    is_live: super::sampler::LiveCheck,
) -> super::sampler::StreamSpec {
    super::sampler::StreamSpec {
        task_id: task_id.to_string(),
        session_id: session_id.to_string(),
        subscriber: subscriber.to_string(),
        max_fps,
        is_live,
    }
}

/// A router over an empty worker: no sessions, no running tasks.
fn empty_router() -> ScreenRouter {
    let runtime = medulla::daemon::DaemonRuntime::new(
        medulla::daemon::DaemonConfig {
            providers: vec![medulla::tinyplace::HarnessProvider::Claude],
            default_provider: medulla::tinyplace::HarnessProvider::Claude,
            workspace: "/tmp".into(),
            env: std::collections::HashMap::new(),
            task_timeout_ms: 1_000,
            capability_timeout_ms: None,
            concurrency: 1,
            status_throttle_ms: 1_000,
            max_pending: 1,
            model: None,
            agent: None,
            extra_args: Vec::new(),
            skip_permissions: false,
            accessible_dirs: Vec::new(),
            router: None,
            custom_harnesses: Vec::new(),
            budget: None,
            attribution: true,
        },
        std::sync::Arc::new(|_| Box::pin(async { Err("unused".to_string()) })),
        std::sync::Arc::new(|_, _| Box::pin(async {})),
    );
    ScreenRouter::new(
        super::super::pty::PtyManager::new(),
        runtime,
        send_fn(|_, _| async {}),
    )
}

#[tokio::test]
async fn a_subscribe_for_a_task_this_sender_never_dispatched_is_refused() {
    // Authorization is structural: the running-task record is keyed by
    // (authenticated sender, task id), so a peer cannot name another's task —
    // the key it would need includes an identity it does not have.
    let mut router = empty_router();
    router.handle(
        "peerA",
        medulla::tinyplace::ScreenMessage::Subscribe {
            task_id: "t1".into(),
            max_fps: 1,
            resync: true,
        },
    );
    assert_eq!(router.active(), 0, "nothing may be streamed");
}

#[tokio::test]
async fn an_unsubscribe_for_a_task_nobody_streams_does_nothing() {
    // Same rule in the other direction: without it, any peer could cancel
    // another's stream by naming its task id.
    let mut router = empty_router();
    router.handle(
        "peerA",
        medulla::tinyplace::ScreenMessage::Unsubscribe {
            task_id: "t1".into(),
        },
    );
    assert_eq!(router.active(), 0);
}

#[tokio::test]
async fn a_kill_for_a_task_this_sender_never_dispatched_is_refused() {
    let mut router = empty_router();
    router.handle(
        "peerA",
        medulla::tinyplace::ScreenMessage::Kill {
            task_id: "t1".into(),
        },
    );
    assert_eq!(router.active(), 0);
}

#[tokio::test]
async fn only_the_peer_a_stream_was_opened_for_can_stop_it() {
    let sessions = super::super::pty::PtyManager::new();
    let send = send_fn(|_, _| async {});
    let mut registry = StreamRegistry::new();
    registry.subscribe(
        &sessions,
        spec("t1", "w_1", "peerA", 1, always_live()),
        send,
    );
    assert_eq!(registry.len(), 1);

    assert!(
        !registry.unsubscribe_for("peerB", "t1"),
        "a stranger naming the task must not stop it"
    );
    assert_eq!(registry.len(), 1, "and the stream survives");

    assert!(registry.unsubscribe_for("peerA", "t1"));
    assert!(registry.is_empty());
}

#[tokio::test]
async fn a_stream_ends_when_its_task_does_even_though_the_session_lives_on() {
    // The case the old rule got wrong. An interactive session outlives the task
    // that ran in it and is handed to the next one, so a stream that watched
    // only the session would carry on — labelling whatever ran next with the
    // task id it was opened for, and sending it to the peer that asked for the
    // *old* task.
    let sessions = super::super::pty::PtyManager::new();
    let send = send_fn(|_, _| async {});
    let live = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(true));
    let is_live = {
        let live = live.clone();
        std::sync::Arc::new(move || live.load(std::sync::atomic::Ordering::SeqCst))
    };

    let handle = spawn_session_stream(
        sessions.clone(),
        spec("t1", "w_1", "peer", 10, is_live),
        send,
    );

    live.store(false, std::sync::atomic::Ordering::SeqCst);
    tokio::time::timeout(std::time::Duration::from_secs(5), handle)
        .await
        .expect("the stream ends once its task is over")
        .expect("and ends cleanly rather than panicking");
}

#[tokio::test]
async fn the_router_ignores_messages_it_is_not_the_receiver_for() {
    // An ack is accepted and does nothing; a frame arriving at the sender is a
    // peer with the protocol backwards. Neither may panic or start a stream.
    let mut router = empty_router();
    router.handle(
        "peerA",
        medulla::tinyplace::ScreenMessage::Ack {
            task_id: "t1".into(),
            seq: 4,
        },
    );
    router.handle(
        "peerA",
        medulla::tinyplace::ScreenMessage::Frame(medulla::tinyplace::ScreenFrame {
            task_id: "t1".into(),
            seq: 1,
            base_seq: 0,
            full: true,
            cols: 1,
            rows: 1,
            cursor: (0, 0),
            hide_cursor: false,
            rows_changed: Vec::new(),
        }),
    );
    assert_eq!(router.active(), 0);
}
