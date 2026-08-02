//! Tests for the screen module.

use super::*;

fn cell(text: &str) -> ScreenCell {
    ScreenCell {
        text: text.into(),
        ..ScreenCell::default()
    }
}

fn snapshot(cells: Vec<Vec<ScreenCell>>) -> ScreenSnapshot {
    ScreenSnapshot {
        cells,
        cursor: (0, 0),
        hide_cursor: false,
    }
}

#[test]
fn default_colours_reset_rather_than_being_forced() {
    // Unstyled harness text must inherit the user's own palette.
    assert_eq!(vt_color(vt100::Color::Default), Color::Reset);
    assert_eq!(vt_color(vt100::Color::Idx(4)), Color::Indexed(4));
    assert_eq!(vt_color(vt100::Color::Rgb(1, 2, 3)), Color::Rgb(1, 2, 3));
}

#[test]
fn inverse_swaps_the_colours_rather_than_setting_reversed() {
    // REVERSED composes inconsistently with an explicit background across
    // terminals, which shows up as invisible text in status bars.
    let inverted = ScreenCell {
        text: "x".into(),
        fg: vt100::Color::Idx(1),
        bg: vt100::Color::Idx(7),
        inverse: true,
        ..ScreenCell::default()
    };
    let style = cell_style(&inverted);
    assert_eq!(style.fg, Some(Color::Indexed(7)));
    assert_eq!(style.bg, Some(Color::Indexed(1)));
    assert!(!style.add_modifier.contains(Modifier::REVERSED));
}

#[test]
fn attributes_become_modifiers() {
    let styled = ScreenCell {
        text: "x".into(),
        bold: true,
        italic: true,
        underline: true,
        ..ScreenCell::default()
    };
    let style = cell_style(&styled);
    assert!(style.add_modifier.contains(Modifier::BOLD));
    assert!(style.add_modifier.contains(Modifier::ITALIC));
    assert!(style.add_modifier.contains(Modifier::UNDERLINED));
}

#[test]
fn a_run_of_equal_style_becomes_one_span() {
    // 120 spans per row would be ~3,600 allocations a frame for no gain.
    let row: Vec<ScreenCell> = "hello".chars().map(|c| cell(&c.to_string())).collect();
    let line = row_line(&row);
    assert_eq!(line.spans.len(), 1);
    assert_eq!(line.spans[0].content, "hello");
}

#[test]
fn a_style_change_splits_the_run() {
    let mut row = vec![cell("a"), cell("b")];
    row.push(ScreenCell {
        text: "c".into(),
        bold: true,
        ..ScreenCell::default()
    });
    let line = row_line(&row);
    assert_eq!(line.spans.len(), 2);
    assert_eq!(line.spans[0].content, "ab");
    assert_eq!(line.spans[1].content, "c");
}

#[test]
fn unstyled_trailing_blanks_are_dropped() {
    let mut row = vec![cell("h"), cell("i")];
    row.extend(std::iter::repeat_with(|| cell(" ")).take(20));
    let line = row_line(&row);
    assert_eq!(line.spans.len(), 1);
    assert_eq!(line.spans[0].content, "hi");
}

#[test]
fn styled_trailing_blanks_survive() {
    // A harness status bar is a run of styled spaces; trimming it would
    // erase the bar.
    let row: Vec<ScreenCell> = std::iter::repeat_with(|| ScreenCell {
        text: " ".into(),
        bg: vt100::Color::Idx(4),
        ..ScreenCell::default()
    })
    .take(10)
    .collect();
    let line = row_line(&row);
    assert_eq!(line.spans.len(), 1);
    assert_eq!(line.spans[0].content, "          ");
}

#[test]
fn one_row_becomes_one_line() {
    let snap = snapshot(vec![vec![cell("a")], vec![cell("b")], vec![cell("c")]]);
    assert_eq!(screen_lines(&snap).len(), 3);
}
