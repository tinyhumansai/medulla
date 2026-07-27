//! The agent-template popup: one template's full declaration, over whatever
//! surface opened it.
//!
//! A declaration is read once and then dismissed — what tools it grants, what
//! model tier it asks for, which harness kinds it may run on, what it was
//! instantiated as. That is popup-shaped work, not pane-shaped: a permanent side
//! pane would spend a third of the width being empty attention while you scan
//! the catalog.
//!
//! The Routing tab's Agent Templates page is the only surface that reaches it;
//! the Agents rail lists what is running, not what could be.

use ratatui::layout::Layout;
use ratatui::layout::{Constraint, Direction, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line as TLine, Span, Text};
use ratatui::widgets::{Block, BorderType, Borders, Clear, Paragraph};
use ratatui::Frame;

use crate::ui::fleet::{fleet_detail, template_rows};

use super::super::types::App;
use super::styled_to_tline;

/// Centre the popup in `area`, leaving the surface behind it visible.
fn popup_area(area: Rect) -> Rect {
    let vertical = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Percentage(10),
            Constraint::Percentage(80),
            Constraint::Percentage(10),
        ])
        .split(area);
    Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Percentage(15),
            Constraint::Percentage(70),
            Constraint::Percentage(15),
        ])
        .split(vertical[1])[1]
}

impl App {
    /// The template the popup is showing, by selection key: whatever the
    /// catalog's cursor sits on.
    ///
    /// `None` closes the popup rather than showing an empty frame — a catalog
    /// that emptied under a stale selection has nothing to say.
    pub(super) fn open_template_key(&self) -> Option<String> {
        let rows = template_rows(&self.fleet_capacity(), &self.fleet_roster());
        rows.get(self.template_index.min(rows.len().saturating_sub(1)))
            .map(|row| row.key.clone())
    }

    /// Draw the agent-template popup over `area`.
    pub(super) fn draw_template_modal(&mut self, f: &mut Frame, area: Rect) {
        let Some(key) = self.open_template_key() else {
            self.template_modal = false;
            return;
        };
        let capacity = self.fleet_capacity();
        let roster = self.fleet_roster();
        let Some(template) = key
            .strip_prefix("template:")
            .and_then(|id| capacity.template(id))
        else {
            self.template_modal = false;
            return;
        };

        let area = popup_area(area);
        let block = Block::default()
            .borders(Borders::ALL)
            .border_type(BorderType::Rounded)
            .border_style(Style::default().fg(self.theme.accent))
            .title(format!(
                " agent template · {} ",
                template.name.clone().unwrap_or_else(|| template.id.clone())
            ));
        let inner = block.inner(area);
        f.render_widget(Clear, area);
        f.render_widget(block, area);

        let width = (inner.width as usize).saturating_sub(2).max(24);
        let mut lines = fleet_detail(&capacity, &roster, &key, width);
        lines.extend(self.template_usage_lines(template.id.as_str(), &roster));

        // One row is kept for the dismiss hint, which must not itself scroll off.
        let height = (inner.height as usize).saturating_sub(1).max(1);
        let max_scroll = lines.len().saturating_sub(height);
        self.template_scroll = self.template_scroll.min(max_scroll);
        let mut view: Vec<TLine> = lines
            .iter()
            .skip(self.template_scroll)
            .take(height)
            .map(styled_to_tline)
            .collect();
        view.push(TLine::from(Span::styled(
            if max_scroll > 0 {
                "PgUp/PgDn scroll · Esc close"
            } else {
                "Esc close"
            },
            Style::default().add_modifier(Modifier::DIM),
        )));
        f.render_widget(Paragraph::new(Text::from(view)), inner);
    }

    /// The agents currently provisioned from this template.
    ///
    /// The catalog says what *may* exist; this says what does. An empty list is
    /// stated rather than omitted — a declared template nothing was stood up
    /// from is exactly what an operator opens this popup to notice.
    fn template_usage_lines(
        &self,
        id: &str,
        roster: &[medulla::runtime::AgentDescriptor],
    ) -> Vec<crate::ui::agents::Line> {
        use crate::ui::agents::Line;
        let provisioned: Vec<&medulla::runtime::AgentDescriptor> = roster
            .iter()
            .filter(|agent| agent.template_id.as_deref() == Some(id))
            .collect();
        let mut lines = vec![
            Line::default(),
            Line {
                text: "provisioned agents".into(),
                color: Some("gray".into()),
                dim: true,
            },
        ];
        if provisioned.is_empty() {
            lines.push(Line {
                text: "  none yet".into(),
                dim: true,
                ..Default::default()
            });
        }
        for agent in provisioned {
            lines.push(Line {
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
