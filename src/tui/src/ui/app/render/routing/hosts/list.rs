//! The tree itself: one row per host, then one per agent under it.

use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line as TLine, Span, Text};
use ratatui::widgets::Paragraph;
use ratatui::Frame;

use medulla::ui::hosts::{HostAgentRow, HostKind, HostRow};

use super::super::super::super::hosts::HostsRow;
use super::super::super::super::types::App;
use super::format::{dim, inline_text};

/// The action hints, split so a narrow terminal still shows the first line.
const FOOTER: &str =
    "↑↓/jk browse · → roles · n new agent · a add host · r refresh · Enter/s select · e edit · d/x remove";

impl App {
    /// Draw the host tree, windowed so the cursor stays visible.
    pub(super) fn draw_host_list(
        &mut self,
        f: &mut Frame,
        area: Rect,
        tree: &[HostRow],
        rows: &[HostsRow],
        selected: usize,
    ) {
        let agents: usize = tree.iter().map(|host| host.agents.len()).sum();
        let block = self.panel(format!("Hosts · {} · agents · {agents}", tree.len()));
        let inner = block.inner(area);
        f.render_widget(block, area);
        let mut lines = Vec::new();
        if rows.is_empty() {
            lines.push(TLine::from(Span::styled(
                "No hosts. This device is not hosting — open Add Host to connect a machine.",
                dim(),
            )));
        } else {
            // Reserve the footer (and optional hub identity) before choosing the
            // window, so the selected row and the action hints stay visible.
            let footer_rows = 1 + usize::from(self.snapshot.link.is_some()) * 2;
            let visible = usize::from(inner.height).saturating_sub(footer_rows).max(1);
            let start = crate::ui::selection::viewport_start(selected, rows.len(), visible);
            for (index, row) in rows.iter().enumerate().skip(start).take(visible) {
                let Some(host) = tree.get(row.host) else {
                    continue;
                };
                let (text, mut style) = match row.agent.and_then(|at| host.agents.get(at)) {
                    Some(agent) => (agent_line(agent), agent_style(agent)),
                    None => (host_line(host), host_style(host)),
                };
                if index == selected {
                    // While the roles toggle has focus the list still marks its
                    // row, but dimly — two lit cursors on one page read as two
                    // selections.
                    style = if self.host_roles_focus {
                        style.add_modifier(Modifier::BOLD)
                    } else {
                        self.theme.selection()
                    };
                }
                lines.push(TLine::from(Span::styled(text, style)));
            }
        }
        if let Some(identity) = &self.snapshot.link {
            lines.push(TLine::from(""));
            lines.push(TLine::from(vec![
                Span::styled("this hub · ", Style::default().fg(Color::Cyan)),
                Span::raw(identity.node_name.clone()),
            ]));
        }
        lines.push(TLine::from(Span::styled(FOOTER, dim())));
        f.render_widget(Paragraph::new(Text::from(lines)), inner);
    }
}

/// A host header: what it is called, where it is, and how many agents it holds.
///
/// A remote host says so on its own row rather than only in the preview: it is
/// the difference between a machine you can declare an agent on and one you can
/// only watch, and that must be legible without moving the cursor.
fn host_line(host: &HostRow) -> String {
    let kind = match host.kind {
        HostKind::Local => "local".to_string(),
        HostKind::Remote => "remote · read-only".to_string(),
    };
    let agents = match host.agents.len() {
        0 => "no agents".to_string(),
        1 => "1 agent".to_string(),
        count => format!("{count} agents"),
    };
    format!(
        "▾ {} · {} · {kind} · {agents}",
        inline_text(&host.label),
        inline_text(&host.id)
    )
}

/// An agent under its host: the id a dispatch targets, its harness, where it
/// works, and the roles it is offered for.
///
/// Indented under the header, and marked when it is the manual default (`●`).
/// A declared agent the roster has no entry for is flagged rather than hidden:
/// it is why nothing is being dispatched to it.
fn agent_line(agent: &HostAgentRow) -> String {
    let mark = if agent.selected { "●" } else { " " };
    let harness = agent
        .harness
        .as_deref()
        .map(|value| format!(" · {}", inline_text(&value.to_uppercase())))
        .unwrap_or_default();
    let workspace = agent
        .workspace
        .as_deref()
        .map(|value| format!(" · {}", inline_text(value)))
        .unwrap_or_default();
    let roles = match agent.roles.len() {
        0 => String::new(),
        1 => " · 1 role".to_string(),
        count => format!(" · {count} roles"),
    };
    let state = match (agent.live, agent.declared) {
        (false, _) => " · declared, not running",
        (true, false) => " · undeclared",
        (true, true) => "",
    };
    format!(
        "   {mark} {}{harness}{workspace}{roles}{state}",
        inline_text(&agent.agent_id)
    )
}

/// A remote host reads dim: it is context, not something to act on.
fn host_style(host: &HostRow) -> Style {
    match host.kind {
        HostKind::Local => Style::default().fg(Color::Cyan),
        HostKind::Remote => dim().fg(Color::Cyan),
    }
}

/// The default agent is green; one that is declared but not running is dim,
/// because it is not a thing the orchestrator can reach right now.
fn agent_style(agent: &HostAgentRow) -> Style {
    if agent.selected {
        Style::default().fg(Color::Green)
    } else if !agent.live {
        dim()
    } else {
        Style::default()
    }
}
