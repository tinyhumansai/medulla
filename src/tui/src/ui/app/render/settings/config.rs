//! The Config subpage: an editor for the settings worth tuning, over a
//! read-only view of the full effective configuration and where it came from.
//!
//! The editor covers the switches and bounded numbers; everything else — paths,
//! URLs, model names, peers — remains file-managed and is shown below so the
//! effective values are still inspectable without leaving the app.

use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line as TLine, Span, Text};
use ratatui::widgets::{Paragraph, Wrap};
use ratatui::Frame;

use super::super::super::settings_edit::SettingKind;
use super::super::super::types::App;

impl App {
    /// Draw the Config subpage: the editable rows, then the effective config.
    pub(super) fn draw_config(&mut self, f: &mut Frame, area: Rect) {
        let rows = self.config_rows();
        // One line per setting, plus a blank line, the selected row's help, and
        // the key hint — all inside a bordered panel.
        let editor_height = rows.len() as u16 + 5;
        let split = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Length(editor_height), Constraint::Min(0)])
            .split(area);

        self.draw_config_editor(f, split[0]);
        self.draw_config_effective(f, split[1]);
    }

    /// Draw the editable settings list.
    fn draw_config_editor(&mut self, f: &mut Frame, area: Rect) {
        let dim = Style::default().add_modifier(Modifier::DIM);
        let rows = self.config_rows();
        let selected = self.config_row_index();

        let mut lines: Vec<TLine> = Vec::new();
        for (i, row) in rows.iter().enumerate() {
            let value = self.read_setting(row);
            let style = if i == selected {
                self.theme.selection()
            } else {
                Style::default()
            };
            let marker = if i == selected { "▸ " } else { "  " };
            // Toggles read as a switch; numbers as a value between stepper
            // arrows, so which control a row offers is obvious at a glance.
            let rendered = match row.kind {
                SettingKind::Toggle => format!("[ {:<3} ]", value.display()),
                SettingKind::Count { .. } => format!("‹ {:>7} ›", value.display()),
            };
            lines.push(TLine::from(vec![
                Span::styled(marker, style),
                Span::styled(format!("{:<22}", row.label), style),
                Span::styled(rendered, style),
            ]));
        }
        if rows.is_empty() {
            lines.push(TLine::from(Span::styled("No editable settings.", dim)));
        }

        lines.push(TLine::from(""));
        if let Some(row) = rows.get(selected) {
            lines.push(TLine::from(Span::styled(row.help, dim)));
            lines.push(TLine::from(Span::styled(
                format!("{}.{}", row.section, row.key),
                dim,
            )));
        }
        let saves_to = match &self.config_path {
            Some(p) => format!(
                "↑↓ select · ←→ change · Enter toggle · saves to {}",
                p.display()
            ),
            None => {
                "↑↓ select · ←→ change · Enter toggle · applies live (no config path set)".into()
            }
        };
        lines.push(TLine::from(Span::styled(saves_to, dim)));

        f.render_widget(
            Paragraph::new(Text::from(lines))
                .wrap(Wrap { trim: false })
                .block(self.panel("Settings")),
            area,
        );
    }

    /// Draw the read-only effective configuration and the files it came from.
    fn draw_config_effective(&mut self, f: &mut Frame, area: Rect) {
        let sources = if self.loaded.sources.is_empty() {
            "built-in defaults".to_string()
        } else {
            self.loaded.sources.join(" < ")
        };
        let body = format!("Sources: {sources}\n\n{}", self.loaded.pretty_json());
        let block = self.panel(format!("Effective configuration · {}", self.loaded.path));
        f.render_widget(
            Paragraph::new(body).wrap(Wrap { trim: false }).block(block),
            area,
        );
    }
}
