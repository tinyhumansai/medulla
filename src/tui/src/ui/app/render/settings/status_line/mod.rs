//! The Status line subpage: the switches beside a live preview of what they do.
//!
//! The **switches** are one group per status-line field, and under each the two
//! or three rows that answer where it sits, when it is drawn, and how it is
//! spelled. The **preview** is three sample harness rows drawn through the real
//! rail renderer, so what a placement costs the fields beside it is shown
//! rather than described.
//!
//! The two used to be stacked on one scrolling page, and the preview is fourteen
//! rows tall: on any terminal shorter than fifty lines that left four of the
//! seven fields below the fold, reachable only by scrolling the preview away —
//! the page put its most useful part and its only navigable part in competition
//! for the same space. Side by side they compete for width instead, which is the
//! axis a terminal has to spare, and a change is visible in the sample without
//! the operator moving or pressing anything.
//!
//! Below a pane wide enough for both halves the layout folds back to two tabs
//! walked with `Tab`. Halving forty columns would leave the rail sample clipped,
//! and a preview that cannot show the row is not one.
//!
//! Under both sits a pinned footer for the selected row: the field it belongs
//! to, what the row does, every value it can take with the current one lit, and
//! the config key the answer is written to. Pinning it is what keeps the
//! explanation from moving out from under the cursor, and moving the field
//! descriptions into it is what let the list above shrink from three lines per
//! group to one.

use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line as TLine, Span, Text};
use ratatui::widgets::{Block, Borders, Paragraph, Wrap};
use ratatui::Frame;

use crate::ui::app::render::sessions::RAIL_MAX_CONTENT;

use super::super::super::status_line::STATUS_LINE_ROW_COUNT;
use super::super::super::types::App;
use super::rendered_height;

mod fields;
mod footer;
mod preview;
#[cfg(test)]
mod tests;

/// The page's two tabs, in the order `Tab` walks them when the pane is too
/// narrow to show both halves at once.
const TABS: [&str; 2] = ["Switches", "Preview"];

/// The narrowest half the page will split into.
///
/// Derived from the preview rather than guessed: the sample frame is the rail's
/// own content width plus its two borders and a column of indent, and the
/// divider the right-hand pane draws costs one more. Anything under this and the
/// rail row — the whole point of the preview — starts losing columns off its
/// end, so the page folds to tabs instead of showing a lie.
const HALF_MIN: u16 = RAIL_MAX_CONTENT as u16 + 4;

impl App {
    /// Draw the Status line subpage: the switches beside (or behind) the
    /// preview, over the pinned detail footer for the selected row.
    pub(super) fn draw_status_line_settings(&mut self, f: &mut Frame, area: Rect) {
        let block = self.content_panel("Status line");
        let inner = block.inner(area);
        f.render_widget(block, area);

        let dim = Style::default().add_modifier(Modifier::DIM);
        let selected = self.status_line_index.min(STATUS_LINE_ROW_COUNT - 1);
        let cfg = self.status_line_config();

        // Recorded for the key handler: `Tab` walks the two tabs only while
        // there are two tabs. Split, it has nothing to switch and belongs to the
        // global tab cycler again.
        self.status_line_split = inner.width >= HALF_MIN * 2;

        // The footer is only worth its height while the rows still have room to
        // be walked, so it never takes more than half the pane. What is cut is
        // cut from the bottom, where the least specific lines sit.
        let footer = self.status_line_footer(selected, &cfg, dim, usize::from(inner.width));
        let footer_height = rendered_height(&footer, inner.width).min(inner.height / 2);

        let body = if self.status_line_split {
            let split = Layout::default()
                .direction(Direction::Vertical)
                .constraints([Constraint::Min(0), Constraint::Length(footer_height)])
                .split(inner);
            let halves = Layout::default()
                .direction(Direction::Horizontal)
                .constraints([Constraint::Percentage(50), Constraint::Percentage(50)])
                .split(split[0]);

            // A rule between the halves, drawn as the right pane's left border
            // so it cannot drift out of step with either column's height.
            let divider = Block::default()
                .borders(Borders::LEFT)
                .border_style(Style::default().fg(self.theme.dim_border));
            let preview = divider.inner(halves[1]);
            f.render_widget(divider, halves[1]);
            f.render_widget(
                Paragraph::new(Text::from(self.status_line_preview(dim))),
                preview.inner(ratatui::layout::Margin {
                    horizontal: 1,
                    vertical: 0,
                }),
            );

            (halves[0], Some(split[1]))
        } else {
            let split = Layout::default()
                .direction(Direction::Vertical)
                .constraints([
                    Constraint::Length(2),
                    Constraint::Min(0),
                    Constraint::Length(footer_height),
                ])
                .split(inner);
            self.draw_status_line_tabs(f, split[0]);
            if self.status_line_preview {
                f.render_widget(
                    Paragraph::new(Text::from(self.status_line_preview(dim))),
                    split[1],
                );
                (Rect::default(), Some(split[2]))
            } else {
                (split[1], Some(split[2]))
            }
        };

        let (switches, footer_area) = body;
        if switches.height > 0 {
            let (lines, selected_line) = self.status_line_fields(selected, &cfg, dim);
            // Scroll only far enough to keep the selected row visible. On a pane
            // tall enough for the whole list this never fires, which is what
            // moving the preview off this column bought.
            let scroll = selected_line
                .saturating_add(1)
                .saturating_sub(usize::from(switches.height))
                .min(usize::from(u16::MAX)) as u16;
            f.render_widget(
                Paragraph::new(Text::from(lines)).scroll((scroll, 0)),
                switches,
            );
        }

        if let Some(footer_area) = footer_area {
            if footer_height > 0 {
                // Wrapped, because a help sentence has to survive a pane
                // narrowed by the nav; the rows above cannot wrap without
                // breaking the scroll arithmetic, but the footer is never
                // scrolled.
                f.render_widget(
                    Paragraph::new(Text::from(footer)).wrap(Wrap { trim: false }),
                    footer_area,
                );
            }
        }
    }

    /// Draw the tab strip: the selected tab in the selection style, the other
    /// dimmed, and the key that moves between them.
    fn draw_status_line_tabs(&mut self, f: &mut Frame, area: Rect) {
        let dim = Style::default().add_modifier(Modifier::DIM);
        let selected = usize::from(self.status_line_preview);
        let mut spans: Vec<Span<'static>> = Vec::new();
        for (index, tab) in TABS.iter().enumerate() {
            let style = if index == selected {
                self.theme.selection()
            } else {
                dim
            };
            spans.push(Span::styled(format!(" {tab} "), style));
            spans.push(Span::raw(" "));
        }
        spans.push(Span::styled("  Tab switches", dim));
        f.render_widget(
            Paragraph::new(Text::from(vec![TLine::from(spans), TLine::from("")])),
            area,
        );
    }
}
