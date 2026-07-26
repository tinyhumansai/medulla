//! The Templates page: the catalog of agent kinds that may be provisioned onto
//! the fleet, with the full declaration of the selected one.
//!
//! A template is not a level of the containment chain, which is why it has a
//! page rather than a branch of the Fleet tree. The chain says *where* an agent
//! can be stood up; a template says *what* may be stood up there — a
//! description (a prompt surface, written like a tool description), default
//! tools, an abstract model tier, and optional per-harness overrides whose mere
//! presence restricts which harness kinds the template may run on.

use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line as TLine, Span, Text};
use ratatui::widgets::Paragraph;
use ratatui::Frame;

use crate::ui::fleet::{fleet_detail, template_rows};
use crate::ui::util::clip;

use super::super::super::types::App;
use super::super::{color, styled_to_tline};

impl App {
    /// Draw the template catalog and the selected template's declaration.
    pub(super) fn draw_templates(&mut self, f: &mut Frame, area: Rect) {
        let capacity = self.fleet_capacity();
        let roster = self.fleet_roster();
        let rows = template_rows(&capacity, &roster);
        let selected = self.template_index.min(rows.len().saturating_sub(1));
        self.template_index = selected;

        let cols = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Percentage(40), Constraint::Min(0)])
            .split(area);

        let block = self.panel(format!("Templates · {}", rows.len()));
        let inner = block.inner(cols[0]);
        f.render_widget(block, cols[0]);

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
        } else {
            let capacity_rows = (inner.height as usize).saturating_sub(1).max(1);
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
            lines.push(TLine::from(Span::styled(
                "↑↓/jk browse · PgUp/PgDn scroll · r refresh",
                Style::default().add_modifier(Modifier::DIM),
            )));
        }
        f.render_widget(Paragraph::new(Text::from(lines)), inner);

        // Detail pane: the same renderer the Fleet page uses, so a template
        // reads identically whichever surface reached it.
        let key = rows
            .get(selected)
            .map(|r| r.key.clone())
            .unwrap_or_default();
        let title = rows
            .get(selected)
            .map(|r| format!("template · {}", r.label))
            .unwrap_or_else(|| "Template".into());
        let block = self.panel(title);
        let inner = block.inner(cols[1]);
        f.render_widget(block, cols[1]);
        let width = (inner.width as usize).saturating_sub(2).max(24);
        let mut detail = fleet_detail(&capacity, &roster, &key, width);
        detail.extend(self.template_usage_lines(&key, &roster));
        let height = (inner.height as usize).max(1);
        let max_scroll = detail.len().saturating_sub(height);
        self.template_scroll = self.template_scroll.min(max_scroll);
        let view: Vec<TLine> = detail
            .iter()
            .skip(self.template_scroll)
            .take(height)
            .map(styled_to_tline)
            .collect();
        f.render_widget(Paragraph::new(Text::from(view)), inner);
    }

    /// The agents currently provisioned from the selected template.
    ///
    /// The catalog says what *may* exist; this says what does. An empty list is
    /// stated rather than omitted — a declared template nothing was stood up
    /// from is exactly what an operator comes to this page to notice.
    fn template_usage_lines(
        &self,
        key: &str,
        roster: &[medulla::runtime::AgentDescriptor],
    ) -> Vec<crate::ui::agents::Line> {
        let Some(id) = key.strip_prefix("template:") else {
            return Vec::new();
        };
        let provisioned: Vec<&medulla::runtime::AgentDescriptor> = roster
            .iter()
            .filter(|agent| agent.template_id.as_deref() == Some(id))
            .collect();
        let mut lines = vec![
            crate::ui::agents::Line::default(),
            crate::ui::agents::Line {
                text: "provisioned agents".into(),
                color: Some("gray".into()),
                dim: true,
            },
        ];
        if provisioned.is_empty() {
            lines.push(crate::ui::agents::Line {
                text: "  none yet".into(),
                dim: true,
                ..Default::default()
            });
        }
        for agent in provisioned {
            lines.push(crate::ui::agents::Line {
                text: format!(
                    "  {} · {}",
                    if agent.name.trim().is_empty() {
                        &agent.id
                    } else {
                        &agent.name
                    },
                    agent.availability
                ),
                color: Some("magenta".into()),
                dim: false,
            });
        }
        lines
    }
}
