//! The pinned detail footer: what the selected row does, every value it can
//! take, and where the answer is written.
//!
//! The choice list is the reason the footer exists. A row shows one value, and a
//! value alone tells an operator nothing about what else is available or what
//! `←/→` is about to do — they would have to press it and watch. Listing the set
//! with the current member highlighted answers both without a keystroke.

use medulla::config::StatusLineConfig;
use ratatui::style::Style;
use ratatui::text::{Line as TLine, Span, Text};
use ratatui::widgets::{Paragraph, Wrap};

use crate::ui::app::status_line::STATUS_LINE_ROWS;
use crate::ui::app::types::App;

/// Return the number of terminal rows the rendered, wrapped footer occupies.
///
/// The page renders its footer through this exact `Paragraph` configuration.
/// Asking Ratatui for its line count avoids duplicating its wrapping semantics,
/// notably for words wider than the available pane.
pub(super) fn rendered_height(lines: &[TLine<'_>], width: u16) -> u16 {
    Paragraph::new(Text::from(lines.to_vec()))
        .wrap(Wrap { trim: false })
        .line_count(width)
        .min(usize::from(u16::MAX)) as u16
}

impl App {
    /// Build the footer lines for the selected row, most specific first.
    ///
    /// Ordered so that truncating from the bottom on a short pane drops the
    /// least specific lines: the explanation and the choices outlive the key
    /// hints, which outlive the file the change lands in.
    pub(super) fn status_line_footer(
        &self,
        selected: usize,
        cfg: &StatusLineConfig,
        dim: Style,
        width: usize,
    ) -> Vec<TLine<'static>> {
        let row = STATUS_LINE_ROWS[selected.min(STATUS_LINE_ROWS.len() - 1)];
        let (value, _) = row.field.value(cfg);

        let mut lines = vec![
            TLine::from(Span::styled("─".repeat(width), dim)),
            TLine::from(Span::styled(row.help, dim)),
        ];

        let mut spans: Vec<Span<'static>> = Vec::new();
        for (index, choice) in row.field.choices().into_iter().enumerate() {
            if index > 0 {
                spans.push(Span::styled(" · ", dim));
            }
            let style = if choice == value {
                self.theme.selection()
            } else {
                dim
            };
            spans.push(Span::styled(choice, style));
        }
        lines.push(TLine::from(spans));

        lines.push(TLine::from(Span::styled(
            format!("statusLine.{} · j/k select · ←/→ change", row.field.key()),
            dim,
        )));
        // Config is layered, so a higher-precedence file can still override what
        // was just written; an operator whose change did not stick needs to know
        // which file was tried. Clipped from the front rather than wrapped: a
        // wrapped path would push itself off a footer sized in whole lines, and
        // the tail is the half that identifies the file anyway.
        lines.push(TLine::from(Span::styled(
            match &self.config_path {
                Some(path) => {
                    let text = format!("saved to {}", path.display());
                    medulla::ui::util::clip_left(&text, width)
                }
                None => "changes apply live (no config path set)".into(),
            },
            dim,
        )));

        lines
    }
}
