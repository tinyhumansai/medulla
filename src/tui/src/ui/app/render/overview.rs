//! The Overview tab: the logo, the session/orchestration/tiny.place panels, the
//! model-routing panel, and the live-activity feed.

use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line as TLine, Span, Text};
use ratatui::widgets::Paragraph;
use ratatui::Frame;

use crate::ui::stream;
use crate::ui::util::clip;

use super::super::types::App;

impl App {
    /// Draw the Overview tab: logo, top panels, model routing, and live activity.
    pub(super) fn draw_overview(&mut self, f: &mut Frame, area: Rect) {
        let rows = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(4),
                Constraint::Length(7),
                Constraint::Length(5),
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
        let top = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([
                Constraint::Percentage(33),
                Constraint::Percentage(33),
                Constraint::Percentage(34),
            ])
            .split(rows[0]);

        // Session panel.
        let mut session = vec![
            TLine::from(format!("id {}", clip(&self.snapshot.session_id, 24))),
            TLine::from(format!(
                "turns {}",
                self.snapshot.messages.len().div_ceil(2)
            )),
            TLine::from(Span::styled(
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
            )),
        ];
        session.push(if self.snapshot.async_mode {
            TLine::from(Span::styled(
                "async ● on",
                Style::default().fg(Color::Magenta),
            ))
        } else {
            TLine::from(Span::styled(
                "async ○ off",
                Style::default().add_modifier(Modifier::DIM),
            ))
        });
        session.push(if self.snapshot.tracing {
            TLine::from(Span::styled(
                "langfuse ● tracing",
                Style::default().fg(Color::Green),
            ))
        } else {
            TLine::from(Span::styled(
                "langfuse ○ off",
                Style::default().add_modifier(Modifier::DIM),
            ))
        });
        f.render_widget(
            Paragraph::new(Text::from(session)).block(self.panel("Session")),
            top[0],
        );

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
        let orch = vec![
            TLine::from(format!("passes {passes}")),
            TLine::from(format!("agents {completed}")),
            TLine::from(format!("active model calls {running_calls}")),
        ];
        f.render_widget(
            Paragraph::new(Text::from(orch)).block(self.panel("Orchestration")),
            top[1],
        );

        // Third panel: tinyplace or opencode.
        self.draw_overview_third(f, top[2]);

        // Model routing: inference is server-managed, so show the runtime we
        // are attached to plus the models actually observed on the stream.
        let workers_val = if let Some(tp) = &self.loaded.config.tinyplace {
            format!("tiny.place · {} peer(s)", tp.peers.len())
        } else {
            self.loaded
                .config
                .opencode
                .as_ref()
                .map(|o| o.model.clone())
                .unwrap_or_default()
        };
        let mut routing = vec![TLine::from(vec![
            Span::styled("runtime ", Style::default().fg(self.theme.primary)),
            Span::raw(self.runtime.describe()),
        ])];
        for (label, tier, color) in [
            ("orchestrator ", "orchestrator", Color::Yellow),
            ("reasoning ", "reasoning", Color::Yellow),
            ("summarizer ", "compress", Color::Blue),
        ] {
            routing.push(TLine::from(vec![
                Span::styled(label, Style::default().fg(color)),
                Span::raw(
                    stream::observed_model(&self.snapshot.events, tier)
                        .unwrap_or("—")
                        .to_string(),
                ),
            ]));
        }
        routing.push(TLine::from(vec![
            Span::styled("workers ", Style::default().fg(Color::Magenta)),
            Span::raw(workers_val),
        ]));
        f.render_widget(
            Paragraph::new(Text::from(routing)).block(self.panel("Model routing")),
            rows[1],
        );

        // Live activity.
        let take = self.visible_count().saturating_sub(7).max(5);
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
            rows[2],
        );
    }

    /// The Overview tab's third top panel: the tiny.place presence summary, or
    /// the OpenCode worker configuration when tiny.place is not enabled.
    pub(super) fn draw_overview_third(&self, f: &mut Frame, area: Rect) {
        if let Some(tp) = &self.loaded.config.tinyplace {
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
            let all_sessions: Vec<_> = self.snapshot.sessions.values().flatten().collect();
            let live = all_sessions.iter().filter(|s| s.state != "ended").count();
            let mut lines = vec![TLine::from(tp.base_url.clone())];
            if readings > 0 {
                lines.push(TLine::from(Span::styled(
                    format!("agents {online}/{} online", peers.len()),
                    Style::default().fg(if online > 0 { Color::Green } else { Color::Red }),
                )));
            } else {
                lines.push(TLine::from(format!(
                    "agents {} · presence pending",
                    peers.len()
                )));
            }
            if !all_sessions.is_empty() {
                lines.push(TLine::from(format!(
                    "sessions {live} live / {} known",
                    all_sessions.len()
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
            f.render_widget(
                Paragraph::new(Text::from(lines)).block(self.panel("tiny.place")),
                area,
            );
        } else {
            let oc = self.loaded.config.opencode.clone().unwrap_or_default();
            let lines = vec![
                TLine::from(oc.model),
                TLine::from(format!("agent {}", oc.agent)),
                TLine::from(format!("concurrency {}", oc.max_concurrency)),
            ];
            f.render_widget(
                Paragraph::new(Text::from(lines)).block(self.panel("OpenCode workers")),
                area,
            );
        }
    }
}
