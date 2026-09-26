//! Bottom-anchored scrollback with a clamped offset.
//!
//! A chat transcript is read from its end: the newest turn is the one being
//! waited on. So the viewport is pinned to the bottom and `scroll` counts lines
//! *back* from there, which is the direction the keys move.
//!
//! The clamping is the reason this is shared rather than three lines inlined at
//! each call site. Saturating the start index at zero looks correct and is not:
//! a held `PageUp` past the top keeps incrementing an offset the render ignores,
//! and every one of those presses then has to be given back before `PageDown`
//! moves anything. [`draw_transcript`] reports the offset it actually used so
//! the caller can store that instead.

use ratatui::layout::Rect;
use ratatui::text::{Line as TLine, Text};
use ratatui::widgets::Paragraph;
use ratatui::Frame;

use super::types::TranscriptFit;

/// Draw `lines` bottom-anchored in `area`, scrolled back by `scroll` rows.
///
/// Returns the offset actually used and how many lines remain below the
/// viewport. Write the offset back onto the caller's state; the count is what a
/// "more below" hint reports.
pub(crate) fn draw_transcript(
    f: &mut Frame,
    area: Rect,
    lines: Vec<TLine<'static>>,
    scroll: usize,
) -> TranscriptFit {
    if area.height == 0 || area.width == 0 {
        return TranscriptFit::default();
    }
    let height = area.height as usize;
    // Everything that does not fit is what there is to scroll back through.
    let overflow = lines.len().saturating_sub(height);
    let scroll = scroll.min(overflow);
    let start = overflow - scroll;
    let visible: Vec<TLine> = lines.into_iter().skip(start).take(height).collect();
    f.render_widget(Paragraph::new(Text::from(visible)), area);
    TranscriptFit {
        scroll,
        below: scroll,
    }
}
