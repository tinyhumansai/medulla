//! The preview under the tree: everything about the row the cursor is on.
//!
//! Two shapes, because the two row kinds answer different questions. A **host**
//! preview is the machine — the capacity, readiness and budgets its capability
//! probe reported, and whether an agent may be declared on it from here. An
//! **agent** preview is the thing a dispatch targets — its harness, workspace,
//! session bound, and the role toggles that are this page's one real edit.

use ratatui::layout::Rect;
use ratatui::style::{Color, Style};
use ratatui::text::{Line as TLine, Span, Text};
use ratatui::widgets::Paragraph;
use ratatui::Frame;

use medulla::runtime::WorkerInfo;
use medulla::ui::hosts::{HostAgentRow, HostKind, HostRow};

use super::super::super::super::hosts::HostsRow;
use super::super::super::super::types::App;
use super::format::{budget_summary, dim, format_bytes, inline_text, readiness_summary};

impl App {
    /// Draw the preview pane for the selected row.
    pub(super) fn draw_host_preview(
        &mut self,
        f: &mut Frame,
        area: Rect,
        tree: &[HostRow],
        row: HostsRow,
    ) {
        let Some(host) = tree.get(row.host) else {
            return;
        };
        let title = match row.agent.and_then(|at| host.agents.get(at)) {
            Some(agent) => {
                // Named agents lead with the name; the id stays beside it,
                // because that is what a dispatch and every status line use.
                let label = agent.label.trim();
                if label.is_empty() || label == agent.agent_id.trim() {
                    format!("Agent · {}", inline_text(&agent.agent_id))
                } else {
                    format!(
                        "Agent · {} · {}",
                        inline_text(label),
                        inline_text(&agent.agent_id)
                    )
                }
            }
            None => format!("Host · {}", inline_text(&host.label)),
        };
        let block = self.panel(title);
        let inner = block.inner(area);
        f.render_widget(block, area);
        // Draw to the height the pane actually got, which is what lets the role
        // list scroll rather than run off the bottom of a short terminal.
        let lines = self.preview_lines_within(tree, row, Some(inner.height as usize));
        f.render_widget(Paragraph::new(Text::from(lines)), inner);
    }

    /// The preview at its natural height, used to size the pane.
    pub(super) fn preview_lines(&self, tree: &[HostRow], row: HostsRow) -> Vec<TLine<'static>> {
        self.preview_lines_within(tree, row, None)
    }

    /// How many rows the preview would draw in `budget` of them. Test seam for
    /// the one property the budget exists to hold: it is never exceeded.
    #[cfg(test)]
    pub(in crate::ui::app) fn preview_height_within(
        &self,
        tree: &[HostRow],
        row: HostsRow,
        budget: usize,
    ) -> usize {
        self.preview_lines_within(tree, row, Some(budget)).len()
    }

    /// Build the preview body, windowing the role list when `budget` rows is
    /// less than it needs. Shared with the height calculation so the pane is
    /// sized to what it will actually draw rather than a guess.
    fn preview_lines_within(
        &self,
        tree: &[HostRow],
        row: HostsRow,
        budget: Option<usize>,
    ) -> Vec<TLine<'static>> {
        let Some(host) = tree.get(row.host) else {
            return Vec::new();
        };
        match row.agent.and_then(|at| host.agents.get(at)) {
            Some(agent) => self.agent_preview(host, agent, budget),
            None => self.host_preview(host),
        }
    }

    /// The machine: where it is, what it has, and what may be done to it here.
    fn host_preview(&self, host: &HostRow) -> Vec<TLine<'static>> {
        let mut lines = vec![TLine::from(vec![
            Span::styled("address    ", dim()),
            Span::raw(inline_text(&host.id)),
        ])];
        let probe = host
            .detail_worker
            .as_deref()
            .and_then(|id| self.runtime.workers().into_iter().find(|w| w.id == id));
        lines.push(TLine::from(vec![
            Span::styled("capacity   ", dim()),
            Span::raw(capacity_line(probe.as_ref())),
        ]));
        if let Some(note) = probe.as_ref().and_then(|w| readiness_summary(&w.readiness)) {
            lines.push(TLine::from(vec![
                Span::styled("harnesses  ", dim()),
                Span::raw(note),
            ]));
        }
        if let Some(note) = probe.as_ref().and_then(|w| budget_summary(&w.budgets)) {
            lines.push(TLine::from(vec![
                Span::styled("budgets    ", dim()),
                Span::raw(note),
            ]));
        }
        // The capability split, stated where the operator is looking. A remote
        // host's agents are declared on that machine — this end can watch them
        // and dispatch to them, and that is all (spec §2.4).
        let (note, style) = match host.kind {
            HostKind::Local if host.agents.is_empty() => (
                "none declared here · n declares one".to_string(),
                Style::default().fg(Color::Green),
            ),
            HostKind::Local => (
                format!("{} declared here · n declares another", host.agents.len()),
                Style::default().fg(Color::Green),
            ),
            HostKind::Remote if host.agents.is_empty() => (
                "none known · they are declared on that machine".to_string(),
                dim(),
            ),
            HostKind::Remote => (
                format!(
                    "{} known to the roster · declared on that machine",
                    host.agents.len()
                ),
                dim(),
            ),
        };
        lines.push(TLine::from(vec![
            Span::styled("agents     ", dim()),
            Span::styled(note, style),
        ]));
        if host.kind == HostKind::Remote {
            // Honest about the gap rather than papering over it: the host link
            // does not exchange declared agent lists yet, so what is listed is
            // whatever this hub happens to have in its own roster.
            lines.push(TLine::from(Span::styled(
                "           this hub lists what its roster reaches, not that machine's declarations",
                dim(),
            )));
        }
        lines
    }

    /// The agent: what runs where, how many sessions it may hold, and its roles.
    fn agent_preview(
        &self,
        host: &HostRow,
        agent: &HostAgentRow,
        budget: Option<usize>,
    ) -> Vec<TLine<'static>> {
        let mut lines = vec![
            TLine::from(vec![
                Span::styled("host       ", dim()),
                Span::raw(format!(
                    "{} · {}",
                    inline_text(&host.label),
                    match host.kind {
                        HostKind::Local => "local",
                        HostKind::Remote => "remote · read-only",
                    }
                )),
            ]),
            TLine::from(vec![
                Span::styled("harness    ", dim()),
                Span::raw(
                    agent
                        .harness
                        .as_deref()
                        .map(inline_text)
                        .unwrap_or_else(|| "not reported".into()),
                ),
            ]),
            TLine::from(vec![
                Span::styled("workspace  ", dim()),
                Span::raw(
                    agent
                        .workspace
                        .as_deref()
                        .map(inline_text)
                        .unwrap_or_else(|| "not reported".into()),
                ),
            ]),
        ];
        let sessions = match agent.max_sessions {
            Some(1) => "1 at a time · checkout".to_string(),
            Some(max) => format!("{max} at a time"),
            None => "not declared here".to_string(),
        };
        lines.push(TLine::from(vec![
            Span::styled("sessions   ", dim()),
            Span::raw(sessions),
        ]));
        let state = match (agent.declared, agent.live) {
            (true, true) => "declared · in the roster",
            (true, false) => "declared · not running",
            (false, true) => "in the roster · not declared here",
            (false, false) => "not declared, not running",
        };
        lines.push(TLine::from(vec![
            Span::styled("state      ", dim()),
            Span::raw(state),
        ]));
        // Whatever rows the detail above did not use are the role list's to fill.
        let role_budget = budget.map(|rows| rows.saturating_sub(lines.len()));
        lines.extend(self.role_lines(agent, role_budget));
        lines
    }

    /// The role toggle list. Roles come from the agent-template catalog, so an
    /// agent can only be offered for a role this hub actually knows how to
    /// brief.
    ///
    /// `budget` caps the rows; the window follows the cursor so a role can never
    /// be selected but off-screen. A remote agent gets the summary and no
    /// checkboxes: its roles are assigned on the machine that declares it.
    ///
    /// The cap is a hard one — the result never exceeds `budget`. A zero budget
    /// returns nothing at all and a budget of one returns only the summary,
    /// because the alternative is drawing past the bottom of the pane, which on
    /// a short terminal clipped the role cursor: the row the operator was about
    /// to toggle was the one that fell off.
    fn role_lines(&self, agent: &HostAgentRow, budget: Option<usize>) -> Vec<TLine<'static>> {
        if budget == Some(0) {
            return Vec::new();
        }
        // What is left once the summary below has taken its row.
        let remaining = budget.map(|rows| rows.saturating_sub(1));
        // The summary leads, because "none assigned" is the state most agents
        // are in and it must not read as one excluded from every role. Trailing
        // it under a dozen checkboxes buried exactly the line that says
        // otherwise.
        let summary = if agent.roles.is_empty() {
            "none assigned · offered for any role".to_string()
        } else {
            agent
                .roles
                .iter()
                .map(|role| inline_text(role))
                .collect::<Vec<_>>()
                .join(", ")
        };
        let mut lines = vec![TLine::from(vec![
            Span::styled("roles      ", dim()),
            Span::styled(
                summary,
                if agent.roles.is_empty() {
                    dim()
                } else {
                    Style::default().fg(Color::Green)
                },
            ),
        ])];
        if !agent.editable {
            if remaining != Some(0) {
                lines.push(TLine::from(vec![
                    Span::styled("           ", dim()),
                    Span::styled(
                        "read-only · assign roles on that machine".to_string(),
                        dim(),
                    ),
                ]));
            }
            return lines;
        }
        let templates = self.agent_templates();
        if templates.is_empty() {
            if remaining != Some(0) {
                lines.push(TLine::from(vec![
                    Span::styled("           ", dim()),
                    Span::styled("no agent templates are declared".to_string(), dim()),
                ]));
            }
            return lines;
        }
        let visible = remaining.unwrap_or(templates.len()).min(templates.len());
        if visible == 0 {
            // One row left, and the summary has it. Better the sentence that
            // says what the agent is offered for than one checkbox out of a
            // dozen, which reads as the whole list.
            return lines;
        }
        let start =
            crate::ui::selection::viewport_start(self.host_role_index, templates.len(), visible);
        for (index, template) in templates.iter().enumerate().skip(start).take(visible) {
            let assigned = agent.roles.iter().any(|role| role == &template.id);
            let mark = if assigned { "[x]" } else { "[ ]" };
            let cursor = if self.host_roles_focus && index == self.host_role_index {
                "▸"
            } else {
                " "
            };
            let mut style = if assigned {
                Style::default().fg(Color::Green)
            } else {
                dim()
            };
            if self.host_roles_focus && index == self.host_role_index {
                style = self.theme.selection();
            }
            lines.push(TLine::from(vec![
                Span::styled("           ", dim()),
                Span::styled(format!("{cursor} {mark} {}", template.id), style),
            ]));
        }
        lines
    }
}

/// The machine's resources, or why they are not known yet.
///
/// A host with nothing in the roster is the ordinary state of a declared host
/// that is not running — it has no probe to read, which is a different answer
/// from a probe that came back empty.
fn capacity_line(probe: Option<&WorkerInfo>) -> String {
    let Some(probe) = probe else {
        return "nothing in the roster reports for this host".into();
    };
    match (
        probe.ip_address.as_deref(),
        probe.cpu_cores,
        probe.memory_available_bytes,
        probe.memory_total_bytes,
    ) {
        (None, None, None, None) => "details not captured · press r to refresh".into(),
        (ip, cpu, available, total) => format!(
            "IP {} · CPU {} · RAM {} available / {} total",
            ip.map(inline_text).unwrap_or_else(|| "unknown".into()),
            cpu.map(|cores| format!("{cores} cores"))
                .unwrap_or_else(|| "unknown".into()),
            available
                .map(format_bytes)
                .unwrap_or_else(|| "unknown".into()),
            total.map(format_bytes).unwrap_or_else(|| "unknown".into()),
        ),
    }
}
