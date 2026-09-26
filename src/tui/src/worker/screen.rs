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
#[path = "screen_tests.rs"]
mod tests;
