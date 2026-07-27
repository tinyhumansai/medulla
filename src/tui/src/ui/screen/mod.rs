//! Rendering a remote worker's synchronised screen into a ratatui pane.
//!
//! The orchestrator's counterpart to [`crate::worker::screen`], which does the
//! same job for a session running locally. The two differ only in what they
//! start from: the worker has emulator cells, the hub has the wire model those
//! cells were coalesced into. They must agree about what a screen *looks* like,
//! so the styling rules here deliberately mirror that module's — most of all
//! the inverse-video one, where disagreeing would show as invisible text in a
//! harness status bar on one view and not the other.
//!
//! No state and no I/O, so the mapping is testable against literal grids.

use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};

use medulla::tinyplace::{
    Color as WireColor, RunStyle, ScreenGrid, ScreenRun, ATTR_BOLD, ATTR_INVERSE, ATTR_ITALIC,
    ATTR_UNDERLINE,
};

/// Convert a wire colour to a ratatui one.
///
/// `Default` becomes [`Color::Reset`] so unstyled text inherits the *viewer's*
/// palette rather than one the sending machine picked.
pub fn wire_color(color: WireColor) -> Color {
    match color {
        WireColor::Default => Color::Reset,
        WireColor::Idx(i) => Color::Indexed(i),
        WireColor::Rgb(r, g, b) => Color::Rgb(r, g, b),
    }
}

/// The ratatui style for one run.
///
/// Inverse is applied by swapping foreground and background rather than with
/// [`Modifier::REVERSED`] — terminals disagree about how REVERSED composes with
/// an explicit background, which shows up as invisible text in status bars. The
/// wire format carries it as a flag precisely so the decision can be made here,
/// at paint time, exactly as the local renderer makes it.
pub fn run_style(style: &RunStyle) -> Style {
    let (fg, bg) = if style.has(ATTR_INVERSE) {
        (wire_color(style.bg), wire_color(style.fg))
    } else {
        (wire_color(style.fg), wire_color(style.bg))
    };
    let mut out = Style::default().fg(fg).bg(bg);
    if style.has(ATTR_BOLD) {
        out = out.add_modifier(Modifier::BOLD);
    }
    if style.has(ATTR_ITALIC) {
        out = out.add_modifier(Modifier::ITALIC);
    }
    if style.has(ATTR_UNDERLINE) {
        out = out.add_modifier(Modifier::UNDERLINED);
    }
    out
}

/// One row of runs as a styled line.
fn row_line(runs: &[ScreenRun]) -> Line<'static> {
    Line::from(
        runs.iter()
            .map(|run| Span::styled(run.text.clone(), run_style(&run.style)))
            .collect::<Vec<_>>(),
    )
}

/// Convert a synchronised screen into ratatui lines, top to bottom.
///
/// Rows arrive already coalesced into runs by the sender, so there is nothing to
/// merge here — a row is a handful of spans, not a span per cell.
pub fn grid_lines(grid: &ScreenGrid) -> Vec<Line<'static>> {
    grid.lines.iter().map(|runs| row_line(runs)).collect()
}

/// The pane title for a watched screen: which harness, whose session, how fresh.
///
/// Staleness is stated rather than implied. A screen that has not changed and a
/// stream that has died look identical, and only one of them means the worker
/// needs attention.
pub fn screen_title(session_id: &str, seq: i64, age_ms: i64) -> String {
    let age = if age_ms < 1_000 {
        format!("{age_ms}ms")
    } else if age_ms < 60_000 {
        format!("{:.1}s", age_ms as f64 / 1_000.0)
    } else {
        format!("{}m", age_ms / 60_000)
    };
    format!("{session_id} · seq {seq} · {age} ago")
}

#[cfg(test)]
mod tests;
