//! The Overview tab: the logo, the this-device/orchestration panels, the local
//! host strip, and the live-activity feed.

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
                Constraint::Length(5),
                Constraint::Length(7),
                Constraint::Min(0),
            ])
            .split(area);
        // The band is one row taller than the art so the wordmark gets a blank
        // line of breathing room under the header/tab strip instead of butting
        // straight up against it.
        let logo: Vec<TLine> = std::iter::once(TLine::from(""))
            .chain(crate::ui::LOGO.iter().map(|row| {
                TLine::from(Span::styled(
                    *row,
                    Style::default()
                        .fg(self.theme.primary)
                        .add_modifier(Modifier::BOLD),
                ))
            }))
            .collect();
        f.render_widget(Paragraph::new(Text::from(logo)), rows[0]);
        let rows = &rows[1..];
        // One row of panels: what this device is running, what the orchestration
        // is doing with it, and — only while this process is hosting — what it
        // is hosting. The old Session panel restated the worker harness's state
        // a column over, so those two are one panel now, and the host strip is a
        // third column rather than a band of its own so it costs the live feed
        // no rows at all.
        let hosting = self.host_obs.is_some();
        let top = Layout::default()
            .direction(Direction::Horizontal)
            .constraints(if hosting {
                [
                    Constraint::Percentage(33),
                    Constraint::Percentage(33),
                    Constraint::Percentage(34),
                ]
                .to_vec()
            } else {
                [Constraint::Percentage(50), Constraint::Percentage(50)].to_vec()
            })
            .split(rows[0]);

        self.draw_this_device(f, top[0]);

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
            top[1],
        );

        // The third column, when this device is also hosting the work.
        if hosting {
            self.draw_host_panel(f, top[2]);
        }

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

    /// The "Local host" column: what this machine will run for the fleet, where,
    /// and what it has run so far.
    ///
    /// Present only while a host is actually running in this process, as the
    /// third panel of the top row. The panel answers the question an operator
    /// asks first — "is my own laptop part of the fleet, and is it doing
    /// anything?" — which was previously only answerable by reading the
    /// orchestrator log. Its neighbour "This device" covers the same machine as
    /// a worker rather than as a host.
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
                    host.address().to_string(),
                    Style::default().add_modifier(Modifier::BOLD),
                ),
                Span::styled(
                    format!("  {providers}"),
                    Style::default().fg(self.theme.primary),
                ),
            ]),
            // Clipped to the panel: a deep workspace path is common and wrapping
            // it would push the counters out of a fixed-height strip.
            TLine::from(Span::styled(
                clip(host.workspace(), area.width.saturating_sub(4) as usize),
                Style::default().add_modifier(Modifier::DIM),
            )),
            TLine::from(format!(
                "running {running} · done {} · failed {} · probes {}",
                stats.tasks_completed, stats.tasks_failed, stats.probes_answered
            )),
        ];
        if let Some(status) = &stats.last_status {
            lines.push(TLine::from(Span::styled(
                clip(
                    &format!("▸ {status}"),
                    area.width.saturating_sub(4) as usize,
                ),
                Style::default().add_modifier(Modifier::DIM),
            )));
        }
        f.render_widget(
            Paragraph::new(Text::from(lines)).block(self.panel("Local host")),
            area,
        );
    }

    /// The Overview tab's left panel: what this machine is running right now —
    /// the live session, the worker harness driving it, and whether the run is
    /// being traced.
    pub(super) fn draw_this_device(&self, f: &mut Frame, area: Rect) {
        let worker = self.loaded.config.opencode.clone().unwrap_or_default();
        let lines = vec![
            TLine::from(vec![
                Span::styled(
                    if self.snapshot.running {
                        "● running"
                    } else {
                        "● idle"
                    },
                    Style::default().fg(if self.snapshot.running {
                        Color::Yellow
                    } else {
                        Color::Green
                    }),
                ),
                Span::styled(
                    format!(
                        " · {} turns · {}",
                        self.snapshot.messages.len().div_ceil(2),
                        clip(&self.snapshot.session_id, 16)
                    ),
                    Style::default().add_modifier(Modifier::DIM),
                ),
            ]),
            TLine::from(vec![
                Span::styled("harness ", Style::default().fg(Color::Magenta)),
                Span::raw(worker_harness_label(&worker.command)),
            ]),
            TLine::from(if worker.model.is_empty() {
                "model —".to_string()
            } else {
                format!("model {}", worker.model)
            }),
            TLine::from(format!(
                "agent {} · concurrency {}",
                worker.agent, worker.max_concurrency
            )),
            if self.snapshot.tracing {
                TLine::from(Span::styled(
                    "langfuse ● tracing",
                    Style::default().fg(Color::Green),
                ))
            } else {
                TLine::from(Span::styled(
                    "langfuse ○ off",
                    Style::default().add_modifier(Modifier::DIM),
                ))
            },
        ];
        f.render_widget(
            Paragraph::new(Text::from(lines)).block(self.panel("This device")),
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

/// The display name for the worker harness a `command` invokes.
///
/// Medulla drives several coding-agent CLIs, so the label is derived from the
/// configured command rather than hard-coded. A recognized command basename maps
/// to its product name ("claude" → "Claude Code"); anything else is shown
/// verbatim, since a custom or wrapped binary is still worth naming.
fn worker_harness_label(command: &str) -> String {
    let basename = command.rsplit(['/', '\\']).next().unwrap_or(command).trim();
    if basename.is_empty() {
        return "—".to_string();
    }
    medulla::tinyplace::frames::HarnessProvider::from_wire(basename)
        .map(|p| p.display_name().to_string())
        .unwrap_or_else(|| basename.to_string())
}
