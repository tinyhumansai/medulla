//! The Agent Templates page: the catalog of agent kinds that may be provisioned
//! onto the fleet, with the selected one's full declaration in a popup.
//!
//! A template is not a level of the containment chain, which is why it has a
//! page rather than a branch of the fleet tree. The chain says *where* an agent
//! can be stood up; a template says *what* may be stood up there — a description
//! (a prompt surface, written like a tool description), default tools, an
//! abstract model tier, and optional per-harness overrides whose mere presence
//! restricts which harness kinds the template may run on.
//!
//! The list is what you scan; the declaration is what you read once. So the list
//! gets the whole page and the declaration opens over it on `Enter`, rather than
//! a permanent side pane that is empty attention most of the time.

use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line as TLine, Span, Text};
use ratatui::widgets::{Paragraph, Wrap};
use ratatui::Frame;

use crate::ui::fleet::template_rows;
use crate::ui::util::clip;

use super::super::super::types::App;
use super::super::color;

impl App {
    /// Draw the agent-template catalog.
    pub(super) fn draw_templates(&mut self, f: &mut Frame, area: Rect) {
        let capacity = self.fleet_capacity();
        let rows = template_rows(&capacity, &self.fleet_roster());
        let selected = self.template_index.min(rows.len().saturating_sub(1));
        self.template_index = selected;

        let block = self.panel(format!("Agent Templates · {}", rows.len()));
        let inner = block.inner(area);
        f.render_widget(block, area);

        let mut lines: Vec<TLine> = Vec::new();
        if rows.is_empty() {
            // No catalog is a supported configuration, not a broken one: agent
            // provisioning keeps its free-text kind and nothing is restricted.
            lines.push(TLine::from(Span::styled(
                "No agent templates declared.",
                Style::default().add_modifier(Modifier::DIM),
            )));
            lines.push(TLine::from(Span::styled(
                "Provisioning stays unrestricted until a catalog exists.",
                Style::default().add_modifier(Modifier::DIM),
            )));
            lines.push(TLine::from(""));
            lines.push(TLine::from(Span::styled(
                format!(
                    "i installs the built-in coding roles into {}",
                    self.template_store_dir().display()
                ),
                Style::default().add_modifier(Modifier::DIM),
            )));
        } else {
            let capacity_rows = (inner.height as usize).saturating_sub(2).max(1);
            let start = crate::ui::selection::viewport_start(selected, rows.len(), capacity_rows);
            for (offset, row) in rows.iter().skip(start).take(capacity_rows).enumerate() {
                let mut style = Style::default().fg(color(row.kind.color()));
                if row.degraded {
                    style = style.add_modifier(Modifier::DIM);
                }
                if start + offset == selected {
                    style = self.theme.selection();
                }
                lines.push(TLine::from(Span::styled(
                    clip(
                        &format!("{} · {}", row.label, row.detail),
                        (inner.width as usize).max(8),
                    ),
                    style,
                )));
            }
            lines.push(TLine::from(""));
            lines.push(TLine::from(Span::styled(
                "↑↓/jk browse · Enter open · i install defaults · r refresh",
                Style::default().add_modifier(Modifier::DIM),
            )));
        }
        f.render_widget(
            Paragraph::new(Text::from(lines)).wrap(Wrap { trim: false }),
            inner,
        );
    }
}
