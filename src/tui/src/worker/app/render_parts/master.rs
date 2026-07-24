//! Master pairing and worker-credential view.

use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{Paragraph, Wrap};
use ratatui::Frame;

use super::super::types::WorkerApp;
use super::dim;

impl WorkerApp {
    /// Draw configured masters beside this worker's connection identity.
    pub(super) fn draw_master(&mut self, f: &mut Frame, area: Rect) {
        let masters = self.master_rows();
        let columns = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Percentage(42), Constraint::Percentage(58)])
            .split(area);

        let selected = self.master_index.min(masters.len().saturating_sub(1));
        self.master_index = selected;
        let block = self.panel(format!("Master · {}", masters.len()), true);
        let inner = block.inner(columns[0]);
        f.render_widget(block, columns[0]);
        let accepted = self.accepted_contacts();
        let mut lines = Vec::new();
        if masters.is_empty() {
            lines.push(dim("No master connected."));
            lines.push(dim(""));
            lines.push(dim("Press a and paste its @handle or worker address."));
            lines.push(dim(
                "This worker requests contact; approval stays explicit.",
            ));
        } else {
            for (index, peer) in masters.iter().enumerate() {
                let address = peer.address.as_deref().unwrap_or(&peer.id);
                let online = accepted.iter().any(|contact| contact.agent_id == address);
                let label = peer
                    .name
                    .as_deref()
                    .or(peer.handle.as_deref())
                    .unwrap_or(address);
                let mut style =
                    Style::default().fg(if online { Color::Green } else { Color::Yellow });
                if index == selected {
                    style = style.add_modifier(Modifier::REVERSED);
                }
                lines.push(Line::from(Span::styled(
                    format!("{} {label}", if online { "●" } else { "◌" }),
                    style,
                )));
                lines.push(dim(&format!("  {address}")));
            }
        }
        f.render_widget(
            Paragraph::new(Text::from(lines)).wrap(Wrap { trim: false }),
            inner,
        );

        let block = self.panel("Worker identity", false);
        let inner = block.inner(columns[1]);
        f.render_widget(block, columns[1]);
        let address = self.agent_id.as_deref().unwrap_or("identity unavailable");
        let endpoint = self.endpoint.as_deref().unwrap_or("relay unavailable");
        let details = vec![
            Line::from(vec![
                Span::styled("address  ", Style::default().fg(Color::DarkGray)),
                Span::raw(address.to_string()),
            ]),
            Line::from(vec![
                Span::styled("relay    ", Style::default().fg(Color::DarkGray)),
                Span::raw(endpoint.to_string()),
            ]),
            Line::from(vec![
                Span::styled("wallet   ", Style::default().fg(Color::DarkGray)),
                Span::raw(self.credential_dir.display().to_string()),
            ]),
            Line::from(""),
            Line::from(Span::styled(
                "Worker-level credentials",
                Style::default()
                    .fg(Color::Cyan)
                    .add_modifier(Modifier::BOLD),
            )),
            dim("This daemon signs and decrypts with its own local wallet."),
            dim("It never borrows the master's backend token or account login."),
            Line::from(""),
            dim("a connect master · i interact · y copy worker address"),
        ];
        f.render_widget(
            Paragraph::new(Text::from(details)).wrap(Wrap { trim: false }),
            inner,
        );
    }
}
