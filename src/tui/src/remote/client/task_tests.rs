//! Unit tests for the connection task's screen de-duplication.
//!
//! A sibling file rather than an inline module: `task.rs` is a single-file leaf
//! module, and this repo puts such a module's unit tests in a dedicated
//! `_tests.rs` beside it (`AGENTS.md`).

use super::next_screen_update;
use crate::worker::pty::ScreenSnapshot;

/// A trivial but distinguishable-only-by-content-equal snapshot: every
/// session started at the same size renders exactly this until something
/// happens in it, which is the case that hid the bug.
fn blank() -> ScreenSnapshot {
    ScreenSnapshot {
        cells: vec![vec![crate::worker::pty::ScreenCell {
            text: crate::worker::pty::CellText::from(" "),
            fg: vt100::Color::Default,
            bg: vt100::Color::Default,
            bold: false,
            italic: false,
            underline: false,
            inverse: false,
        }]],
        cursor: (0, 0),
        hide_cursor: false,
    }
}

#[test]
fn the_first_frame_for_a_session_always_publishes() {
    let mut last = None;
    let update = next_screen_update(&mut last, Some("w_1"), Some(blank()));
    assert_eq!(update.map(|(id, _)| id), Some("w_1".to_string()));
}

#[test]
fn an_identical_frame_for_the_same_session_is_suppressed() {
    let mut last = None;
    next_screen_update(&mut last, Some("w_1"), Some(blank()));
    let update = next_screen_update(&mut last, Some("w_1"), Some(blank()));
    assert!(
        update.is_none(),
        "an idle watched session should cost nothing"
    );
}

#[test]
fn switching_to_a_session_with_an_identical_grid_still_publishes() {
    // The bug: two fresh shells render the same blank prompt, so comparing
    // only the snapshot made switching from w_1 to w_2 look like "nothing
    // changed" and the pane never got a first frame for w_2.
    let mut last = None;
    next_screen_update(&mut last, Some("w_1"), Some(blank()));
    let update = next_screen_update(&mut last, Some("w_2"), Some(blank()));
    assert_eq!(
        update.map(|(id, _)| id),
        Some("w_2".to_string()),
        "a session switch must publish even when the grid looks the same"
    );
}

#[test]
fn losing_the_watch_clears_the_cache_for_the_next_one() {
    let mut last = None;
    next_screen_update(&mut last, Some("w_1"), Some(blank()));
    assert!(next_screen_update(&mut last, None, None).is_none());
    // Re-watching the very same session after the gap must publish again,
    // rather than being compared against the stale cache from before the
    // gap.
    let update = next_screen_update(&mut last, Some("w_1"), Some(blank()));
    assert_eq!(update.map(|(id, _)| id), Some("w_1".to_string()));
}
