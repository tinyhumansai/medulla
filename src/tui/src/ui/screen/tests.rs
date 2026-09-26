//! Unit tests for the remote-screen pane renderer.
//!
//! The rules here have to match `crate::worker::screen`'s, since the two render
//! the same screen from different sides. Where that matters — inverse video,
//! and inheriting the viewer's palette — it is asserted rather than assumed.

use super::*;

fn styled(attrs: u8) -> RunStyle {
    RunStyle {
        attrs,
        ..RunStyle::default()
    }
}

#[test]
fn default_colours_inherit_the_viewers_palette() {
    // Not the sending machine's: a hub rendering a worker's screen must not
    // force colours the worker never chose.
    assert_eq!(wire_color(WireColor::Default), Color::Reset);
    assert_eq!(wire_color(WireColor::Idx(4)), Color::Indexed(4));
    assert_eq!(wire_color(WireColor::Rgb(1, 2, 3)), Color::Rgb(1, 2, 3));
}

#[test]
fn inverse_swaps_the_colours_rather_than_setting_reversed() {
    // The same rule the worker's own renderer applies. REVERSED composes
    // inconsistently with an explicit background across terminals, which shows
    // as invisible text in a harness status bar.
    let style = run_style(&RunStyle {
        fg: WireColor::Idx(1),
        bg: WireColor::Idx(7),
        attrs: ATTR_INVERSE,
    });
    assert_eq!(style.fg, Some(Color::Indexed(7)));
    assert_eq!(style.bg, Some(Color::Indexed(1)));
    assert!(!style.add_modifier.contains(Modifier::REVERSED));
}

#[test]
fn attributes_become_modifiers() {
    let style = run_style(&styled(ATTR_BOLD | ATTR_ITALIC | ATTR_UNDERLINE));
    assert!(style.add_modifier.contains(Modifier::BOLD));
    assert!(style.add_modifier.contains(Modifier::ITALIC));
    assert!(style.add_modifier.contains(Modifier::UNDERLINED));
}

#[test]
fn each_row_becomes_one_line_and_each_run_one_span() {
    let grid = ScreenGrid {
        cols: 6,
        rows: 2,
        lines: vec![
            vec![
                ScreenRun::plain("ab"),
                ScreenRun::new("cd", styled(ATTR_BOLD)),
            ],
            vec![ScreenRun::plain("ef")],
        ],
        cursor: (0, 0),
        hide_cursor: false,
    };
    let lines = grid_lines(&grid);
    assert_eq!(lines.len(), 2);
    assert_eq!(lines[0].spans.len(), 2);
    assert_eq!(lines[0].spans[0].content, "ab");
    assert_eq!(lines[0].spans[1].content, "cd");
    assert!(lines[0].spans[1]
        .style
        .add_modifier
        .contains(Modifier::BOLD));
    assert_eq!(lines[1].spans.len(), 1);
}

#[test]
fn an_empty_row_renders_as_an_empty_line_not_a_missing_one() {
    // Blank rows are dropped to nothing by the sender's coalescing. They still
    // have to occupy their row, or everything below shifts up.
    let grid = ScreenGrid {
        cols: 4,
        rows: 3,
        lines: vec![
            vec![ScreenRun::plain("top")],
            Vec::new(),
            vec![ScreenRun::plain("end")],
        ],
        cursor: (0, 0),
        hide_cursor: false,
    };
    let lines = grid_lines(&grid);
    assert_eq!(lines.len(), 3, "the blank row still takes a line");
    assert!(lines[1].spans.is_empty());
    assert_eq!(lines[2].spans[0].content, "end");
}

#[test]
fn an_empty_grid_renders_nothing_without_panicking() {
    assert!(grid_lines(&ScreenGrid::default()).is_empty());
}

#[test]
fn the_title_states_how_stale_the_screen_is() {
    // A still screen and a dead stream look identical; only the age tells them
    // apart, so it is in the title rather than inferred.
    assert_eq!(screen_title("w_3", 418, 400), "w_3 · seq 418 · 400ms ago");
    assert_eq!(screen_title("w_3", 418, 2_500), "w_3 · seq 418 · 2.5s ago");
    assert_eq!(screen_title("w_3", 418, 185_000), "w_3 · seq 418 · 3m ago");
}
