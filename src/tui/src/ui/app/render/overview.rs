//! The Overview tab: the logo, the this-device and orchestration panels, and
//! the live-activity feed.

use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line as TLine, Span, Text};
use ratatui::widgets::Paragraph;
use ratatui::Frame;

use crate::ui::stream;
use crate::ui::util::clip;

use super::super::types::App;

impl App {
    /// Draw the Overview tab: logo, top panels, and live activity.
    pub(super) fn draw_overview(&mut self, f: &mut Frame, area: Rect) {
        let rows = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                // Exactly the art: no spare row above or below it. The panels
                // below open with a border, which is separation enough.
                Constraint::Length(3),
                Constraint::Length(7),
                Constraint::Min(0),
            ])
            .split(area);
        let logo: Vec<TLine> = crate::ui::LOGO
            .iter()
            .map(|row| {
                TLine::from(Span::styled(
                    *row,
                    Style::default()
                        .fg(self.theme.primary)
                        .add_modifier(Modifier::BOLD),
                ))
            })
            .collect();
        f.render_widget(Paragraph::new(Text::from(logo)), rows[0]);
        let rows = &rows[1..];
        // Two panels: this device, and what the orchestration is doing with it.
        // The device panel is the host observer's, so it is only laid out while
        // this process is actually hosting — otherwise Orchestration takes the
        // whole row rather than leaving half of it empty.
        let hosting = self.host_obs.is_some();
        let top = Layout::default()
            .direction(Direction::Horizontal)
            .constraints(if hosting {
                [Constraint::Percentage(50), Constraint::Percentage(50)].to_vec()
            } else {
                [Constraint::Percentage(100)].to_vec()
            })
            .split(rows[0]);
        let orch_area = if hosting { top[1] } else { top[0] };

        if hosting {
            self.draw_host_panel(f, top[0]);
        }

        // Orchestration panel.
        let running_calls = stream::running_calls(&self.snapshot.events);
        let completed = self
            .snapshot
            .last_result
            .as_ref()
            .map(|r| r.task_ledger.len())
            .unwrap_or(0);
        let passes = self
            .snapshot
            .last_result
            .as_ref()
            .map(|r| r.pass_count.to_string())
            .unwrap_or_else(|| "—".into());
        let mut orch = vec![
            TLine::from(format!("passes {passes}")),
            TLine::from(format!("agents {completed}")),
            TLine::from(format!("active model calls {running_calls}")),
        ];
        let decisions = self.decisions().len();
        if decisions > 0 {
            orch.push(TLine::from(Span::styled(
                format!("decisions: {decisions} · E open"),
                Style::default().fg(Color::Yellow),
            )));
        }
        // tiny.place is a property of the wider run rather than of this device,
        // so its presence summary rides along in this column when enabled.
        orch.extend(self.tinyplace_lines());
        f.render_widget(
            Paragraph::new(Text::from(orch)).block(self.panel("Orchestration")),
            orch_area,
        );

        // Live activity.
        let take = self.visible_count().saturating_sub(1).max(5);
        let start = self.snapshot.events.len().saturating_sub(take);
        let recent: Vec<TLine> = self.snapshot.events[start..]
            .iter()
            .map(|e| self.event_line(e, area.width.saturating_sub(6) as usize, false))
            .collect();
        let body = if recent.is_empty() {
            Text::from(TLine::from(Span::styled(
                "No events yet.",
                Style::default().add_modifier(Modifier::DIM),
            )))
        } else {
            Text::from(recent)
        };
        f.render_widget(
            Paragraph::new(body).block(self.panel("Live activity")),
            rows[1],
        );
    }

    /// The "This Device" column: what this machine will run for the fleet,
    /// where, and what it has run so far.
    ///
    /// Present only while a host is actually running in this process, as the
    /// left panel of the top row. It answers the question an operator asks first
    /// — "is my own laptop part of the fleet, and is it doing anything?" — which
    /// was previously only answerable by reading the orchestrator log.
    ///
    /// One fact per line: the address, the harnesses, the workspace, and the
    /// counters each want the full column width, and cramming the first three
    /// onto one row made the panel unreadable as soon as it became a column
    /// rather than a full-width strip.
    pub(super) fn draw_host_panel(&self, f: &mut Frame, area: Rect) {
        let Some(host) = &self.host_obs else {
            return;
        };
        if area.height == 0 {
            return;
        }
        let stats = host.stats();
        let running = stats.tasks_running();
        let providers = host
            .providers()
            .iter()
            .map(|provider| provider.as_str())
            .collect::<Vec<_>>()
            .join(", ");

        let width = area.width.saturating_sub(4) as usize;
        let mut lines = vec![
            TLine::from(vec![
                Span::styled(
                    if running > 0 {
                        "● busy "
                    } else {
                        "● ready "
                    },
                    Style::default().fg(if running > 0 {
                        Color::Yellow
                    } else {
                        Color::Green
                    }),
                ),
                Span::styled(
                    clip(host.address(), width.saturating_sub(8)),
                    Style::default().add_modifier(Modifier::BOLD),
                ),
            ]),
            TLine::from(Span::styled(
                clip(&providers, width),
                Style::default().fg(self.theme.primary),
            )),
            // Clipped to the panel: a deep workspace path is common and wrapping
            // it would push the counters out of a fixed-height panel.
            TLine::from(Span::styled(
                clip(host.workspace(), width),
                Style::default().add_modifier(Modifier::DIM),
            )),
            TLine::from(format!(
                "running {running} · done {} · failed {} · probes {}",
                stats.tasks_completed, stats.tasks_failed, stats.probes_answered
            )),
        ];
        if let Some(status) = &stats.last_status {
            lines.push(TLine::from(Span::styled(
                clip(&format!("▸ {status}"), width),
                Style::default().add_modifier(Modifier::DIM),
            )));
        }
        f.render_widget(
            Paragraph::new(Text::from(lines)).block(self.panel("This Device")),
            area,
        );
    }

    /// The tiny.place presence summary appended to the Orchestration panel, or
    /// nothing at all when tiny.place is not configured.
    ///
    /// Kept to two lines — peers online and who this node is — because the panel
    /// it joins already spends most of its height on the run's own counters.
    pub(super) fn tinyplace_lines(&self) -> Vec<TLine<'static>> {
        if self.loaded.config.tinyplace.is_none() {
            return Vec::new();
        }
        let peers: Vec<_> = self
            .snapshot
            .roster
            .iter()
            .filter(|a| a.metadata.get("harness").and_then(|v| v.as_str()) == Some("tinyplace"))
            .collect();
        let readings = peers
            .iter()
            .filter(|a| self.snapshot.presence.contains_key(&a.id))
            .count();
        let online = peers
            .iter()
            .filter(|a| {
                self.snapshot
                    .presence
                    .get(&a.id)
                    .map(|p| p.online)
                    .unwrap_or(false)
            })
            .count();
        let mut lines = Vec::new();
        if readings > 0 {
            lines.push(TLine::from(Span::styled(
                format!("tiny.place {online}/{} online", peers.len()),
                Style::default().fg(if online > 0 { Color::Green } else { Color::Red }),
            )));
        } else {
            lines.push(TLine::from(format!(
                "tiny.place {} peers · presence pending",
                peers.len()
            )));
        }
        if let Some(me) = &self.snapshot.tinyplace {
            let who = me.handle.clone().unwrap_or_else(|| clip(&me.agent_id, 24));
            lines.push(TLine::from(format!("me {who}")));
        } else {
            lines.push(TLine::from(Span::styled(
                "me · connecting…",
                Style::default().add_modifier(Modifier::DIM),
            )));
        }
        lines
    }
}
