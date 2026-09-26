//! The pinned detail footer: which field the selected row belongs to and what
//! that field shows, what the row itself does, every value it can take, and
//! where the answer is written.
//!
//! The choice list is the reason the footer exists. A row shows one value, and a
//! value alone tells an operator nothing about what else is available or what
//! `←/→` is about to do — they would have to press it and watch. Listing the set
//! with the current member highlighted answers both without a keystroke.
//!
//! The field description moved here from a line under every heading. It answers
//! a question asked about one field at a time, so paying seven lines of the
//! list for it — on a page whose list did not fit — was the wrong trade.

use medulla::config::StatusLineConfig;
use ratatui::style::Style;
use ratatui::text::{Line as TLine, Span};

use crate::ui::app::status_line::{status_line_group, STATUS_LINE_ROWS};
use crate::ui::app::types::App;

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
        let group = status_line_group(selected);
        let (value, _) = row.field.value(cfg);

        let mut lines = vec![
            TLine::from(Span::styled("─".repeat(width), dim)),
            TLine::from(vec![
                Span::styled(group.title, Style::default().fg(self.theme.accent)),
                Span::styled(format!(" · {}", group.description), dim),
            ]),
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
            format!(
                "statusLine.{} · {}",
                row.field.key(),
                if self.status_line_split {
                    // Both halves are on screen, so there is nothing for `Tab`
                    // to switch and no reason to advertise it.
                    "↑↓ or j/k select · ←/→ change"
                } else if self.status_line_preview {
                    // The rows are not on screen here, but the selection and
                    // `←/→` still work — adjusting a field while watching the
                    // sample redraw is the reason to be on this tab at all.
                    "←/→ change this field · ↑↓ pick another · Tab back"
                } else {
                    "↑↓ or j/k select · ←/→ change · Tab previews"
                }
            ),
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
