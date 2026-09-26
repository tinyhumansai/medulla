//! The Appearance subpage: the switches beside a live preview of what they do.
//!
//! The **switches** are the rows, grouped by the surface each one changes, with
//! a pinned footer explaining whichever row is under the cursor and listing
//! every value it can take. The **preview** is the same settings drawn as the
//! interface they produce: a selected row, an agent asking for attention, the
//! status line's indicators, and the sidebar in whatever grouping and order is
//! configured.
//!
//! Side by side, so a colour or a grouping is judged as it is chosen. Below a
//! pane wide enough for both halves the layout folds back to two tabs walked
//! with `Tab`, because a preview squeezed under its samples' width shows
//! something other than what it is previewing.
//!
//! Splitting the two is what let the prose go. The page used to carry a
//! paragraph under every group heading trying to describe an effect in words —
//! "how Medulla highlights a task waiting for you" — which is both longer than
//! the effect and less convincing than showing it. Now the explanation is one
//! dim line about the row you are actually on, and the effect has a tab of its
//! own.

use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line as TLine, Span, Text};
use ratatui::widgets::{Block, Borders, Paragraph, Wrap};
use ratatui::Frame;

use super::rendered_height;
use crate::ui::app::types::App;

mod footer;
mod options;
mod preview;

/// The page's two tabs, in the order `Tab` walks them when the pane is too
/// narrow to show both halves at once.
const TABS: [&str; 2] = ["Switches", "Preview"];

/// The narrowest half the page will split into: the preview's sample frame plus
/// its indent, borders, and the divider the right-hand pane draws.
const HALF_MIN: u16 = preview::SAMPLE_WIDTH as u16 + 5;

impl App {
    /// Draw Appearance: the switches beside (or behind) the preview, over the
    /// pinned detail footer for the selected row.
    pub(super) fn draw_appearance(&mut self, f: &mut Frame, area: Rect) {
        let block = self.content_panel("Appearance");
        let inner = block.inner(area);
        f.render_widget(block, area);

        let dim = Style::default().add_modifier(Modifier::DIM);

        // Recorded for the key handler: `Tab` walks the two tabs only while
        // there are two tabs.
        self.appearance_split = inner.width >= HALF_MIN * 2;

        // The footer is only worth its height while the rows still have room to
        // be walked, so it never takes more than half the pane.
        let footer = self.appearance_footer(dim, usize::from(inner.width));
        let footer_height = rendered_height(&footer, inner.width).min(inner.height / 2);

        let switches = if self.appearance_split {
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
            self.draw_appearance_preview(f, preview);

            self.draw_appearance_footer(f, footer, footer_height, split[1]);
            halves[0]
        } else {
            let split = Layout::default()
                .direction(Direction::Vertical)
                .constraints([
                    Constraint::Length(2),
                    Constraint::Min(0),
                    Constraint::Length(footer_height),
                ])
                .split(inner);
            self.draw_appearance_tabs(f, split[0]);
            self.draw_appearance_footer(f, footer, footer_height, split[2]);
            if self.appearance_preview {
                self.draw_appearance_preview(f, split[1]);
                Rect::default()
            } else {
                split[1]
            }
        };

        if switches.height > 0 {
            let (lines, selected_line) = self.appearance_options();
            // Scroll only far enough to keep the selected row visible: the first
            // group stays put while the cursor is still near the top.
            let scroll = selected_line
                .saturating_add(1)
                .saturating_sub(usize::from(switches.height))
                .min(usize::from(u16::MAX)) as u16;
            f.render_widget(
                Paragraph::new(Text::from(lines)).scroll((scroll, 0)),
                switches,
            );
        }
    }

    /// Draw the pinned footer, when the pane left it any height.
    fn draw_appearance_footer(
        &mut self,
        f: &mut Frame,
        footer: Vec<TLine<'static>>,
        height: u16,
        area: Rect,
    ) {
        if height > 0 {
            f.render_widget(
                Paragraph::new(Text::from(footer)).wrap(Wrap { trim: false }),
                area,
            );
        }
    }

    /// Draw the tab strip: the selected tab in the selection style, the other
    /// dimmed, and the key that moves between them.
    fn draw_appearance_tabs(&mut self, f: &mut Frame, area: Rect) {
        let dim = Style::default().add_modifier(Modifier::DIM);
        let selected = usize::from(self.appearance_preview);
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

    /// Draw the preview into `area`, indented off the divider.
    fn draw_appearance_preview(&mut self, f: &mut Frame, area: Rect) {
        let area = area.inner(ratatui::layout::Margin {
            horizontal: 1,
            vertical: 0,
        });
        let lines = self.appearance_preview_lines(usize::from(area.width));
        f.render_widget(Paragraph::new(Text::from(lines)), area);
    }
}
