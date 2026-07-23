//! The ratatui render for the worker TUI: chrome, the three tabs, and the
//! embedded harness terminal.

use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{Block, BorderType, Borders, Paragraph, Wrap};
use ratatui::Frame;

use medulla::contacts::{ContactRequest, RequestState};

use super::super::pty::{PtyState, SessionRow};
use super::super::screen::screen_lines;
use super::types::{Screen, WorkerApp, TABS, TAB_CONTACTS, TAB_REQUESTS, TAB_SESSIONS};

#[path = "render_parts/setup.rs"]
mod setup;

impl WorkerApp {
    /// Draw the whole screen.
    pub fn draw(&mut self, f: &mut Frame) {
        if self.screen == Screen::Setup {
            self.draw_setup(f, f.area());
            return;
        }
        let rows = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(1), // header
                Constraint::Length(1), // tab bar
                Constraint::Min(3),    // body
                Constraint::Length(1), // status
            ])
            .split(f.area());

        self.draw_header(f, rows[0]);
        self.draw_tabs(f, rows[1]);
        match self.tab {
            TAB_SESSIONS => self.draw_sessions(f, rows[2]),
            TAB_CONTACTS => self.draw_contacts(f, rows[2]),
            TAB_REQUESTS => self.draw_requests(f, rows[2]),
            _ => {}
        }
        self.draw_status(f, rows[3]);
    }

    /// A titled panel block.
    fn panel<'a>(&self, title: impl Into<String>, focused: bool) -> Block<'a> {
        let border = if focused {
            Color::Cyan
        } else {
            Color::DarkGray
        };
        Block::default()
            .borders(Borders::ALL)
            .border_type(BorderType::Rounded)
            .border_style(Style::default().fg(border))
            .title(Span::styled(
                title.into(),
                Style::default()
                    .fg(Color::Cyan)
                    .add_modifier(Modifier::BOLD),
            ))
    }

    /// The header: what this process is, and its address.
    fn draw_header(&self, f: &mut Frame, area: Rect) {
        let mut spans = vec![
            Span::styled(
                "MEDULLA WORKER",
                Style::default()
                    .fg(Color::Cyan)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::raw("  "),
            Span::styled(
                format!("{} running", self.sessions.running_count()),
                Style::default().fg(Color::Green),
            ),
        ];
        // The daemon's own address is what a peer needs in order to reach it, so
        // it is on screen rather than in a log line scrolled past at startup.
        if let (Some(mode), Some(harness)) = (self.mode, self.harness) {
            spans.push(Span::raw("  "));
            spans.push(Span::styled(
                format!("{} on {}", mode.as_str(), harness.as_str()),
                Style::default().fg(Color::Cyan),
            ));
        }
        if let Some(agent_id) = &self.agent_id {
            spans.push(Span::raw("  "));
            spans.push(Span::styled(
                agent_id.clone(),
                Style::default().fg(Color::DarkGray),
            ));
        } else {
            spans.push(Span::styled(
                "  no tiny.place identity",
                Style::default().fg(Color::Yellow),
            ));
        }
        f.render_widget(Paragraph::new(Line::from(spans)), area);
    }

    /// The tab bar, with a pending-request badge.
    fn draw_tabs(&self, f: &mut Frame, area: Rect) {
        let pending = self.pending_requests().len();
        let mut spans = Vec::new();
        for (i, name) in TABS.iter().enumerate() {
            // In headless mode the first tab is the log, not a session list;
            // labelling it "Sessions" would promise something that never appears.
            let name = if i == TAB_SESSIONS && self.is_headless() {
                "Log"
            } else {
                name
            };
            let mut label = format!(" {} {name} ", i + 1);
            // A waiting request is the one thing on this screen that needs the
            // operator, so it is counted in the chrome rather than only inside
            // its own tab.
            if i == TAB_REQUESTS && pending > 0 {
                label = format!(" {} {name} ({pending}) ", i + 1);
            }
            let style = if i == self.tab {
                Style::default()
                    .fg(Color::Black)
                    .bg(Color::Cyan)
                    .add_modifier(Modifier::BOLD)
            } else if i == TAB_REQUESTS && pending > 0 {
                Style::default().fg(Color::Yellow)
            } else {
                Style::default().fg(Color::DarkGray)
            };
            spans.push(Span::styled(label, style));
        }
        f.render_widget(Paragraph::new(Line::from(spans)), area);
    }

    /// The status line, with the keys for the active context.
    fn draw_status(&self, f: &mut Frame, area: Rect) {
        let hints = if self.confirm.is_some() {
            "y confirm · any other key cancels"
        } else {
            match self.tab {
                TAB_SESSIONS if self.is_headless() => {
                    "↑↓ scroll the log · y copy address · Tab tabs · q quit"
                }
                TAB_SESSIONS => {
                    "↑↓ watch a session · K kill · d drop · y copy address · Tab tabs · q quit"
                }
                TAB_CONTACTS => "↑↓ select · p policy · y copy address · Tab tabs · q quit",
                _ => "↑↓ select · a accept · x decline · B block · r refresh · p policy · y copy · q quit",
            }
        };
        let line = Line::from(vec![
            Span::styled(
                format!(" {} ", self.status),
                Style::default().fg(Color::White),
            ),
            Span::styled(hints, Style::default().fg(Color::DarkGray)),
        ]);
        f.render_widget(Paragraph::new(line), area);
    }

    /// The Sessions tab.
    ///
    /// Headless has no screen to embed — one process per task, and the daemon's
    /// own log is the whole view. Interactive splits the pane between the
    /// session list and the selected session's live terminal, shown read-only.
    fn draw_sessions(&mut self, f: &mut Frame, area: Rect) {
        if self.is_headless() {
            self.draw_daemon_log(f, area);
            return;
        }
        let columns = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Length(34), Constraint::Min(20)])
            .split(area);

        self.draw_session_list(f, columns[0]);
        self.draw_terminal(f, columns[1]);
    }

    /// The daemon's own log — the headless view.
    fn draw_daemon_log(&mut self, f: &mut Frame, area: Rect) {
        let block = self.panel(format!("Daemon log · {} lines", self.logs.len()), true);
        let inner = block.inner(area);
        f.render_widget(block, area);

        let height = inner.height as usize;
        let lines = self.logs.tail(height + self.log_scroll);
        let shown = lines.len().saturating_sub(self.log_scroll);
        let mut rendered: Vec<Line> = Vec::new();
        if lines.is_empty() {
            rendered.push(dim("Waiting for peer work."));
            rendered.push(dim(""));
            rendered.push(dim(
                "Accepted peers can dispatch tasks; each one is logged here.",
            ));
        } else {
            for line in lines.iter().take(shown) {
                rendered.push(Line::from(vec![
                    Span::styled(
                        format!("{} ", crate::ui::util::clock(line.at)),
                        Style::default().add_modifier(Modifier::DIM),
                    ),
                    Span::styled(
                        line.text.clone(),
                        Style::default().fg(log_color(&line.text)),
                    ),
                ]));
            }
        }
        f.render_widget(Paragraph::new(Text::from(rendered)), inner);
    }

    /// The session list.
    fn draw_session_list(&mut self, f: &mut Frame, area: Rect) {
        let rows = self.session_rows();
        let selected = if rows.is_empty() {
            0
        } else {
            self.session_index.min(rows.len() - 1)
        };
        self.session_index = selected;

        let block = self.panel(format!("Sessions · {}", rows.len()), true);
        let inner = block.inner(area);
        f.render_widget(block, area);

        let mut lines: Vec<Line> = Vec::new();
        if rows.is_empty() {
            lines.push(dim("No sessions running."));
            lines.push(dim(""));
            if self.providers.is_empty() {
                // A missing binary is a different problem from an empty list and
                // must not read the same.
                lines.push(Line::from(Span::styled(
                    "No coding agents on PATH.",
                    Style::default().fg(Color::Yellow),
                )));
            } else {
                lines.push(dim("Peer tasks open sessions here."));
            }
        } else {
            let visible = inner.height as usize;
            let start = selected
                .saturating_sub(visible / 2)
                .min(rows.len().saturating_sub(visible));
            for (i, row) in rows.iter().enumerate().skip(start).take(visible) {
                lines.push(session_line(row, i == selected, self.now()));
            }
        }
        f.render_widget(Paragraph::new(Text::from(lines)), inner);
    }

    /// The selected session's live screen, shown read-only.
    fn draw_terminal(&mut self, f: &mut Frame, area: Rect) {
        let Some(row) = self.selected_session() else {
            let block = self.panel("Terminal", false);
            let inner = block.inner(area);
            f.render_widget(block, area);
            f.render_widget(
                Paragraph::new(Text::from(vec![dim("Select a session to watch it.")])),
                inner,
            );
            return;
        };

        let title = format!(
            "{} · {} · {}",
            row.provider.as_str(),
            row.label,
            row.state.as_str(),
        );
        let block = self.panel(title, false);
        let inner = block.inner(area);
        f.render_widget(block, area);

        // Resize the PTY to the pane before reading it, so the harness reflows
        // to what the operator is actually looking at rather than to the
        // geometry it happened to start with.
        self.sessions.resize(&row.id, inner.width, inner.height);
        self.terminal_area = inner;

        let Some(snapshot) = self.sessions.screen_rows(&row.id) else {
            return;
        };
        f.render_widget(Paragraph::new(Text::from(screen_lines(&snapshot))), inner);
        // No cursor is drawn: this pane is a window, not a keyboard. A blinking
        // cursor here would imply typing goes somewhere it does not.
    }

    /// The Contacts tab: peers this daemon has accepted.
    fn draw_contacts(&mut self, f: &mut Frame, area: Rect) {
        let rows = self.accepted_contacts();
        let selected = self.contact_index.min(rows.len().saturating_sub(1));
        self.contact_index = selected;

        let policy = self
            .contacts
            .as_ref()
            .map(|d| d.policy().as_str())
            .unwrap_or("—");
        let block = self.panel(
            format!("Contacts · {} · admission={policy}", rows.len()),
            true,
        );
        let inner = block.inner(area);
        f.render_widget(block, area);

        let mut lines: Vec<Line> = Vec::new();
        if self.contacts.is_none() {
            lines.push(dim("No tiny.place identity is configured, so this"));
            lines.push(dim("daemon has no contact graph. Add a [tinyplace]"));
            lines.push(dim("section to accept work from peers."));
        } else if rows.is_empty() {
            lines.push(dim("No accepted contacts."));
            lines.push(dim("Accept a request on the Requests tab to let a"));
            lines.push(dim("peer send work here."));
        } else {
            for (i, contact) in rows.iter().enumerate() {
                lines.push(contact_line(contact, i == selected));
            }
        }
        f.render_widget(
            Paragraph::new(Text::from(lines)).wrap(Wrap { trim: false }),
            inner,
        );
    }

    /// The Requests tab: peers waiting on a decision.
    fn draw_requests(&mut self, f: &mut Frame, area: Rect) {
        let rows = self.pending_requests();
        let selected = self.request_index.min(rows.len().saturating_sub(1));
        self.request_index = selected;

        let columns = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Percentage(45), Constraint::Percentage(55)])
            .split(area);

        // The header states poll health, so an empty queue is distinguishable
        // from a relay that cannot be reached — they look identical otherwise.
        let health = self
            .contacts
            .as_ref()
            .map(|d| d.health().summary(self.now()))
            .unwrap_or_else(|| "no identity".to_string());
        let block = self.panel(
            format!("Pending requests · {} · {health}", rows.len()),
            true,
        );
        let inner = block.inner(columns[0]);
        f.render_widget(block, columns[0]);

        let mut lines: Vec<Line> = Vec::new();
        if self.contacts.is_none() {
            lines.push(dim("No tiny.place identity is configured."));
        } else if rows.is_empty() {
            lines.push(dim("Nothing waiting."));
        } else {
            for (i, request) in rows.iter().enumerate() {
                lines.push(request_line(request, i == selected));
            }
        }
        f.render_widget(Paragraph::new(Text::from(lines)), inner);

        // The detail pane: what accepting actually grants.
        let block = self.panel("Decision", false);
        let inner = block.inner(columns[1]);
        f.render_widget(block, columns[1]);
        let detail = match self.selected_request() {
            Some(request) => vec![
                Line::from(vec![
                    Span::styled("peer ", Style::default().fg(Color::DarkGray)),
                    Span::raw(request.agent_id.clone()),
                ]),
                Line::from(""),
                Line::from(Span::styled(
                    "Accepting lets this peer send task frames to this",
                    Style::default().fg(Color::Yellow),
                )),
                Line::from(Span::styled(
                    "machine, which run as coding-agent sessions here.",
                    Style::default().fg(Color::Yellow),
                )),
                Line::from(""),
                dim("a accept · x decline · B block"),
            ],
            None => vec![dim("No request selected.")],
        };
        f.render_widget(
            Paragraph::new(Text::from(detail)).wrap(Wrap { trim: false }),
            inner,
        );
    }
}

/// The accent colour for a daemon log line.
///
/// Keyed off the markers the daemon already writes, so the colouring tracks the
/// log's own vocabulary rather than a second one invented here.
fn log_color(text: &str) -> Color {
    if text.contains('✗') || text.contains("failed") || text.contains("error") {
        Color::Red
    } else if text.contains('✓') {
        Color::Green
    } else if text.contains('→') {
        Color::Cyan
    } else {
        Color::Reset
    }
}

/// A dimmed line.
pub(super) fn dim(text: &str) -> Line<'static> {
    Line::from(Span::styled(
        text.to_string(),
        Style::default().add_modifier(Modifier::DIM),
    ))
}

/// The accent colour for a session state.
fn state_color(state: PtyState) -> Color {
    match state {
        PtyState::Running => Color::Green,
        PtyState::Exited { code: Some(0) } | PtyState::Exited { code: None } => Color::DarkGray,
        PtyState::Exited { .. } | PtyState::Failed => Color::Red,
    }
}

/// One row of the session list.
fn session_line(row: &SessionRow, selected: bool, now: i64) -> Line<'static> {
    let idle = row.idle_ms(now);
    // A running session that has said nothing for a while is the signal an
    // operator is looking for, so it is called out rather than left to be
    // inferred from a timestamp.
    let quiet = if row.state.is_running() && idle >= 10_000 {
        format!(" · quiet {}s", idle / 1_000)
    } else {
        String::new()
    };
    let text = format!("{} {}{}", row.state.glyph(), row.label, quiet);
    let mut style = Style::default().fg(state_color(row.state));
    if selected {
        style = style.add_modifier(Modifier::REVERSED);
    }
    Line::from(Span::styled(text, style))
}

/// One row of the contacts list.
fn contact_line(contact: &ContactRequest, selected: bool) -> Line<'static> {
    let text = format!("✓ {}", contact.display_name());
    let mut style = Style::default().fg(Color::Green);
    if selected {
        style = style.add_modifier(Modifier::REVERSED);
    }
    Line::from(Span::styled(text, style))
}

/// One row of the pending-request list.
fn request_line(request: &ContactRequest, selected: bool) -> Line<'static> {
    let text = format!("{} {}", request.state.glyph(), request.display_name());
    let mut style = Style::default().fg(match request.state {
        RequestState::Failed => Color::Red,
        _ => Color::Yellow,
    });
    if selected {
        style = style.add_modifier(Modifier::REVERSED);
    }
    Line::from(Span::styled(text, style))
}
