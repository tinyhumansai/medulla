//! Rendering a harness's live terminal screen inside a ratatui pane.
//!
//! The emulator gives us a grid of cells with VT attributes; ratatui wants
//! styled spans. This module is that translation, and nothing else — it holds no
//! state and does no I/O, so the mapping is unit-testable against literal
//! screens.
//!
//! One row of the harness becomes one [`Line`]. Adjacent cells sharing a style
//! are coalesced into a single [`Span`]: a 120-column screen is 120 cells, and
//! emitting a span per cell would allocate ~3,600 spans per frame at 30 rows for
//! no visual difference.

use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};

use super::pty::{ScreenCell, ScreenSnapshot};

/// Convert a vt100 colour to a ratatui one.
///
/// `Default` maps to [`Color::Reset`] so the harness's unstyled text inherits
/// the terminal's own palette rather than being forced to a colour we picked.
pub fn vt_color(color: vt100::Color) -> Color {
    match color {
        vt100::Color::Default => Color::Reset,
        vt100::Color::Idx(i) => Color::Indexed(i),
        vt100::Color::Rgb(r, g, b) => Color::Rgb(r, g, b),
    }
}

/// The ratatui style for one emulator cell.
///
/// `inverse` is applied by swapping foreground and background rather than with
/// [`Modifier::REVERSED`]: harnesses use inverse video for selections and status
/// bars, and terminals disagree about how REVERSED composes with an explicit
/// background, which produces invisible text on some of them.
pub fn cell_style(cell: &ScreenCell) -> Style {
    let (fg, bg) = if cell.inverse {
        (vt_color(cell.bg), vt_color(cell.fg))
    } else {
        (vt_color(cell.fg), vt_color(cell.bg))
    };
    let mut style = Style::default().fg(fg).bg(bg);
    if cell.bold {
        style = style.add_modifier(Modifier::BOLD);
    }
    if cell.italic {
        style = style.add_modifier(Modifier::ITALIC);
    }
    if cell.underline {
        style = style.add_modifier(Modifier::UNDERLINED);
    }
    style
}

/// Convert a screen snapshot into ratatui lines, coalescing runs of equal style.
pub fn screen_lines(snapshot: &ScreenSnapshot) -> Vec<Line<'static>> {
    snapshot.cells.iter().map(|row| row_line(row)).collect()
}

/// Convert one row of cells into a styled line.
fn row_line(row: &[ScreenCell]) -> Line<'static> {
    let mut spans: Vec<Span<'static>> = Vec::new();
    let mut run = String::new();
    let mut run_style: Option<Style> = None;

    for cell in row {
        let style = cell_style(cell);
        match run_style {
            Some(current) if current == style => run.push_str(&cell.text),
            Some(current) => {
                spans.push(Span::styled(std::mem::take(&mut run), current));
                run.push_str(&cell.text);
                run_style = Some(style);
            }
            None => {
                run.push_str(&cell.text);
                run_style = Some(style);
            }
        }
    }
    if let Some(style) = run_style {
        // Trailing blanks carry no information and cost width on a narrow pane,
        // but only strip them when they are unstyled — a harness's status bar is
        // a run of styled spaces and must survive.
        if style == Style::default().fg(Color::Reset).bg(Color::Reset) {
            let trimmed = run.trim_end();
            if !trimmed.is_empty() {
                spans.push(Span::styled(trimmed.to_string(), style));
            }
        } else {
            spans.push(Span::styled(run, style));
        }
    }
    Line::from(spans)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cell(text: &str) -> ScreenCell {
        ScreenCell {
            text: text.to_string(),
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
}
