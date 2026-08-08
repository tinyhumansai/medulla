//! The Status line settings subpage: a live preview of the harness row, the
//! fields that produce it, and a detail footer for whichever field is selected.
//!
//! The preview is the point of the page. Placement and spelling are choices
//! about a row that is thirty-six columns wide and shares those columns between
//! six fields, so "branch on line 2" is not a question anyone can answer in the
//! abstract — they have to see what it does to the path. It renders through the
//! same [`own_session_lines`](crate::ui::app::App::own_session_lines) the rail
//! itself uses, against sample sessions, so it cannot drift from the real row.
//!
//! The page is laid out in two regions. The upper one scrolls: preview, then the
//! fields, each group headed by the field's name and one line saying what it
//! shows. The lower one is pinned: what the selected row does, every value it
//! can take with the current one highlighted, and where the answer is written.
//! Pinning it is what keeps the explanation from moving out from under the
//! cursor as the operator walks down a list taller than the pane.

use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::Text;
use ratatui::widgets::{Paragraph, Wrap};
use ratatui::Frame;

use super::super::super::status_line::STATUS_LINE_ROW_COUNT;
use super::super::super::types::App;

mod fields;
mod footer;
mod preview;
#[cfg(test)]
mod tests;

impl App {
    /// Draw the Status line subpage: the scrolling preview and fields, over the
    /// pinned detail footer for the selected row.
    pub(super) fn draw_status_line_settings(&mut self, f: &mut Frame, area: Rect) {
        let block = self.panel("Status line");
        let inner = block.inner(area);
        f.render_widget(block, area);

        let dim = Style::default().add_modifier(Modifier::DIM);
        let selected = self.status_line_index.min(STATUS_LINE_ROW_COUNT - 1);
        let cfg = self.status_line_config();

        // The footer is only worth its height while the fields still have room
        // to be walked, so it never takes more than half the pane. What is cut
        // is cut from the bottom, where the least specific lines sit.
        let footer = self.status_line_footer(selected, &cfg, dim, usize::from(inner.width));
        let footer_height = footer::rendered_height(&footer, inner.width).min(inner.height / 2);

        let split = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Min(0), Constraint::Length(footer_height)])
            .split(inner);

        let (lines, selected_line) = self.status_line_fields(selected, &cfg, dim);
        // Scroll only far enough to keep the selected field visible. The preview
        // stays pinned while the upper rows fit, then yields space as the cursor
        // moves toward the bottom of a short terminal.
        let viewport_height = usize::from(split[0].height);
        let scroll = selected_line
            .saturating_add(1)
            .saturating_sub(viewport_height)
            .min(usize::from(u16::MAX)) as u16;
        f.render_widget(
            Paragraph::new(Text::from(lines)).scroll((scroll, 0)),
            split[0],
        );

        if footer_height > 0 {
            // Wrapped, because a help sentence has to survive a pane narrowed by
            // the nav; the fields above cannot wrap without breaking the scroll
            // arithmetic, but the footer is never scrolled.
            f.render_widget(
                Paragraph::new(Text::from(footer)).wrap(Wrap { trim: false }),
                split[1],
            );
        }
    }
}
