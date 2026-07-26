//! The Routing › Fleet page: the declared containment chain as a tree on the
//! left, the selected node's full declaration on the right.
//!
//! The list is an index, not a report — one line per host, harness, workspace,
//! agent, or template, with just enough trailing status to spot the row worth
//! opening. Everything else (every budget window, the workspace profile, a
//! template's instructions) is in the detail pane, which is why selecting a row
//! is worth doing.

use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line as TLine, Span, Text};
use ratatui::widgets::Paragraph;
use ratatui::Frame;

use crate::ui::fleet::fleet_detail;
use crate::ui::util::clip;

use super::super::super::types::App;
use super::super::{color, styled_to_tline};

impl App {
    /// Draw the fleet tree and the selected node's detail.
    pub(super) fn draw_fleet(&mut self, f: &mut Frame, area: Rect) {
        let rows = self.fleet_rows();
        let selected = self.fleet_index.min(rows.len().saturating_sub(1));
        self.fleet_index = selected;

        let cols = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Percentage(46), Constraint::Min(0)])
            .split(area);

        let capacity_snapshot = self.fleet_capacity();
        let hosts = capacity_snapshot.hosts.len();
        let agents = rows
            .iter()
            .filter(|r| r.kind == crate::ui::fleet::FleetNodeKind::Agent)
            .count();
        let block = self.panel(format!(
            "Fleet · {hosts} host{} · {agents} agent{}",
            if hosts == 1 { "" } else { "s" },
            if agents == 1 { "" } else { "s" }
        ));
        let inner = block.inner(cols[0]);
        f.render_widget(block, cols[0]);

        let mut lines: Vec<TLine> = Vec::new();
        if rows.is_empty() {
            // Nothing declared is the resting state for a backend that has not
            // reported capacity yet — say so rather than drawing an empty tree.
            lines.push(TLine::from(Span::styled(
                "No fleet declared. Connect a worker, or declare hosts in your config.",
                Style::default().add_modifier(Modifier::DIM),
            )));
            lines.push(TLine::from(Span::styled(
                "r refreshes from the backend.",
                Style::default().add_modifier(Modifier::DIM),
            )));
        } else {
            let capacity = (inner.height as usize).saturating_sub(1).max(1);
            let start = crate::ui::selection::viewport_start(selected, rows.len(), capacity);
            for (offset, row) in rows.iter().skip(start).take(capacity).enumerate() {
                let mut style = Style::default().fg(color(row.kind.color()));
                if row.degraded {
                    style = style.add_modifier(Modifier::DIM);
                }
                if start + offset == selected {
                    style = self.theme.selection();
                }
                let indent = "  ".repeat(row.depth);
                let text = if row.detail.is_empty() {
                    format!("{indent}{}", row.label)
                } else {
                    format!("{indent}{} · {}", row.label, row.detail)
                };
                lines.push(TLine::from(Span::styled(
                    clip(&text, (inner.width as usize).max(8)),
                    style,
                )));
            }
            lines.push(TLine::from(Span::styled(
                "↑↓/jk browse · r refresh",
                Style::default().add_modifier(Modifier::DIM),
            )));
        }
        f.render_widget(Paragraph::new(Text::from(lines)), inner);

        // Detail pane.
        let key = rows
            .get(selected)
            .filter(|row| row.kind.selectable())
            .map(|row| row.key.clone())
            .unwrap_or_default();
        let title = rows
            .get(selected)
            .map(|row| format!("{} · {}", row.kind.label(), row.label))
            .unwrap_or_else(|| "Detail".into());
        let block = self.panel(title);
        let inner = block.inner(cols[1]);
        f.render_widget(block, cols[1]);
        let width = (inner.width as usize).saturating_sub(2).max(24);
        let detail = fleet_detail(&capacity_snapshot, &self.fleet_roster(), &key, width);
        let height = (inner.height as usize).max(1);
        let max_scroll = detail.len().saturating_sub(height);
        self.fleet_scroll = self.fleet_scroll.min(max_scroll);
        let view: Vec<TLine> = detail
            .iter()
            .skip(self.fleet_scroll)
            .take(height)
            .map(styled_to_tline)
            .collect();
        f.render_widget(Paragraph::new(Text::from(view)), inner);
    }
}
