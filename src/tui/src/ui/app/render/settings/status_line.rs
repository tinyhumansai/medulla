//! The Status line settings subpage: the harness-row field list, and a live
//! preview of what those choices produce.
//!
//! The preview is the point of the page. Placement and spelling are choices
//! about a row that is thirty-six columns wide and shares those columns between
//! five fields, so "branch on line 2" is not a question anyone can answer in the
//! abstract — they have to see what it does to the path. It renders through the
//! same [`own_session_lines`](crate::ui::app::App::own_session_lines) the rail
//! itself uses, against sample sessions, so it cannot drift from the real row.

use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line as TLine, Span, Text};
use ratatui::widgets::Paragraph;
use ratatui::Frame;
use unicode_width::UnicodeWidthStr;

use medulla::protocol::HarnessProvider;

use crate::ui::app::render::agents::RAIL_MAX_CONTENT;
use crate::worker::pty::{PtyState, SessionControl, SessionRow};

use super::super::super::status_line::{STATUS_LINE_ROWS, STATUS_LINE_ROW_COUNT};
use super::super::super::types::App;

/// The column the value is drawn at, so the answers line up in one column
/// regardless of how long each row's label is.
const VALUE_COLUMN: usize = 24;

impl App {
    /// Draw the Status line subpage: the preview, then the editable fields.
    pub(super) fn draw_status_line_settings(&mut self, f: &mut Frame, area: Rect) {
        let block = self.panel("Status line");
        let inner = block.inner(area);
        f.render_widget(block, area);

        let dim = Style::default().add_modifier(Modifier::DIM);
        let bold = Style::default().add_modifier(Modifier::BOLD);
        let selected = self.status_line_index.min(STATUS_LINE_ROW_COUNT - 1);
        let cfg = self.status_line_config();

        let mut lines: Vec<TLine> = Vec::new();
        lines.push(TLine::from(vec![
            Span::styled("Preview", bold),
            Span::styled(
                format!("  · {RAIL_MAX_CONTENT} columns, as on the rail"),
                dim,
            ),
        ]));
        lines.extend(self.status_line_preview(dim));
        lines.push(TLine::from(""));
        lines.push(TLine::from(Span::styled("Fields", bold)));
        let selected_line = lines.len() + selected;

        for (index, row) in STATUS_LINE_ROWS.iter().enumerate() {
            let style = if index == selected {
                self.theme.selection()
            } else {
                Style::default()
            };
            let marker = if index == selected { "▸ " } else { "  " };
            // A qualifier is indented under the field it belongs to, so each
            // group reads as one question with follow-ups rather than as peers.
            let label = if row.is_qualifier {
                format!("  {}", row.label)
            } else {
                row.label.to_string()
            };
            let (value, _) = row.field.value(&cfg);
            lines.push(TLine::from(vec![
                Span::styled(format!("{marker}{label:<VALUE_COLUMN$}"), style),
                Span::styled(value, if index == selected { style } else { dim }),
            ]));
        }

        lines.push(TLine::from(""));
        lines.push(TLine::from(Span::styled(
            "j/k select · ←/→ or Enter change · applies live",
            dim,
        )));
        lines.push(TLine::from(Span::styled(
            match &self.config_path {
                Some(path) => format!("saved to {}", path.display()),
                None => "changes apply live (no config path set)".into(),
            },
            dim,
        )));
        // Scroll only far enough to keep the selected field visible. The
        // preview stays pinned while the upper rows fit, then yields space as
        // the cursor moves toward the bottom of a short terminal.
        let viewport_height = usize::from(inner.height);
        let scroll = selected_line
            .saturating_add(1)
            .saturating_sub(viewport_height)
            .min(usize::from(u16::MAX)) as u16;
        f.render_widget(Paragraph::new(Text::from(lines)).scroll((scroll, 0)), inner);
    }

    /// The preview: three sample harness rows inside a rail-width frame, each
    /// captioned with the condition it stands for.
    ///
    /// Three, not one, because "when selected" and "on alert" are answers about
    /// rows the operator is *not* looking at — a single always-selected, always-
    /// healthy sample would render those two choices unpreviewable, which is the
    /// one thing this page exists to prevent.
    fn status_line_preview(&self, dim: Style) -> Vec<TLine<'static>> {
        let width = RAIL_MAX_CONTENT;
        let rule = |left: &str, right: &str| {
            TLine::from(Span::styled(
                format!("  {left}{}{right}", "─".repeat(width)),
                dim,
            ))
        };

        // One healthy row under the cursor, one healthy row that is not, and
        // one that needs attention: together they preview every visibility.
        let samples = [
            (sample_selected as fn() -> SessionRow, true, "selected"),
            (sample_orchestrator, false, "not selected"),
            (sample_alerting, false, "on alert"),
        ];
        let mut lines = vec![rule("┌", "┐")];
        for (index, (sample, active, caption)) in samples.iter().enumerate() {
            if index > 0 {
                lines.push(rule("├", "┤"));
            }
            let row = sample();
            for (offset, line) in self
                .own_session_lines(&row, *active, width, medulla::clock::now_millis())
                .into_iter()
                .enumerate()
            {
                let used: usize = line.spans.iter().map(|span| span.content.width()).sum();
                let mut spans = vec![Span::styled("  │", dim)];
                spans.extend(line.spans);
                spans.push(Span::styled(
                    format!("{}│", " ".repeat(width.saturating_sub(used))),
                    dim,
                ));
                // Caption the sample once, beside its first line.
                if offset == 0 {
                    spans.push(Span::styled(format!("  {caption}"), dim));
                }
                lines.push(TLine::from(spans));
            }
        }
        lines.push(rule("└", "┘"));
        lines
    }
}

/// A running, operator-held harness in a Git checkout — the common case, and the
/// one whose path is long enough to show what the layout costs it.
fn sample_selected() -> SessionRow {
    SessionRow {
        mcp_grant_session: None,
        id: "preview".into(),
        label: "preview".into(),
        provider: HarnessProvider::Claude,
        preset: None,
        state: PtyState::Running,
        cwd: "/home/you/work/tinyhumans/medulla-public".into(),
        branch: Some("feat/status-line".into()),
        launch_root: None,
        launch_commit: None,
        launch_checkout_identity: None,
        session_id: None,
        thread_name: Some("Ship the status line".into()),
        started_at: 0,
        last_output_at: 0,
        last_error: None,
        busy: false,
        control: SessionControl::User,
        origin: crate::worker::pty::SessionOrigin::User,
        retained: false,
        name: None,
        attention: None,
    }
}

/// A finished, orchestrator-held harness outside a repository — the other half
/// of what the state, control, and branch rows can produce.
fn sample_orchestrator() -> SessionRow {
    SessionRow {
        provider: HarnessProvider::Codex,
        preset: None,
        state: PtyState::Exited { code: Some(0) },
        cwd: "/tmp/scratch".into(),
        branch: None,
        control: SessionControl::Orchestrator,
        ..sample_selected()
    }
}

/// A harness whose child died — what an "on alert" field is waiting for.
fn sample_alerting() -> SessionRow {
    SessionRow {
        state: PtyState::Failed,
        last_error: Some("session exited unexpectedly".into()),
        ..sample_selected()
    }
}
