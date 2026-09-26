//! [`SessionStream`]'s frame decision, alone and folded by a viewer end to end.
//!
//! Pure: a stream is ticked against snapshots built by hand, with no pty and no
//! runtime. The end-to-end property — a viewer folding what a sampler emits
//! ends up holding the emulator's screen — is asserted here too, since it is
//! the only check that covers the conversion and the diff *together*.

use medulla::protocol::{apply_frame, ApplyOutcome, Color, ScreenView, ATTR_BOLD, ATTR_INVERSE};

use crate::worker::pty::ScreenCell;
use crate::worker::stream::convert::wire_grid;
use crate::worker::stream::{sample_interval, SessionStream};

use super::conversion::snapshot;

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
