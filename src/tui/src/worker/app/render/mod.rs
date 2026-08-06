//! The ratatui render for the worker TUI: chrome, the three tabs, and the
//! embedded harness terminal.

use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{Block, Paragraph};
use ratatui::Frame;

use super::super::pty::{PtyState, SessionRow, ATTENTION_GLYPH};
use super::super::screen::screen_lines;
use super::types::{Screen, WorkerApp, TABS, TAB_MASTER, TAB_SESSIONS, TAB_WORKSPACES};

mod master;
mod prompt;
mod setup;
mod status;
mod workspaces;

impl WorkerApp {
    /// Draw the whole screen.
    pub fn draw(&mut self, f: &mut Frame) {
        self.hit_rows = None;
        self.hit_setup = None;
        if self.screen == Screen::Setup {
            self.draw_setup(f, f.area());
            return;
        }
        let rows = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(2), // header (identity, then what it serves)
                Constraint::Length(1), // tab bar
                Constraint::Min(3),    // body
                Constraint::Length(1), // status
            ])
            .split(f.area());

        self.draw_header(f, rows[0]);
        self.draw_tabs(f, rows[1]);
        match self.tab {
            TAB_SESSIONS => self.draw_sessions(f, rows[2]),
            TAB_MASTER => self.draw_master(f, rows[2]),
            TAB_WORKSPACES => self.draw_workspaces(f, rows[2]),
            _ => {}
        }
        self.draw_status(f, rows[3]);
        if self.prompt.is_some() {
            self.draw_prompt(f, f.area());
        }
    }

    /// A titled panel block.
    fn panel<'a>(&self, title: impl Into<String>, focused: bool) -> Block<'a> {
        crate::ui::widgets::panel(&self.theme, title, focused)
    }

    /// The header: what this process is and where it can be reached, then what
    /// it will actually do for a peer that reaches it.
    ///
    /// The second line exists because the first answers "who am I" and an
    /// operator's next three questions are "what can this run", "in which
    /// directory", and "through which relay". The workspace especially: a peer's
    /// task edits files there, and it was previously only visible by inferring
    /// it from the shell the daemon was launched in.
    fn draw_header(&self, f: &mut Frame, area: Rect) {
        let rows = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Length(1), Constraint::Length(1)])
            .split(area);
        self.draw_header_identity(f, rows[0]);
        self.draw_header_capacity(f, rows[1]);
    }

    /// The second header line: harnesses, workspace, relay, and approved peers.
    fn draw_header_capacity(&self, f: &mut Frame, area: Rect) {
        let dim = Style::default().add_modifier(Modifier::DIM);
        let mut spans = Vec::new();

        // What this machine can actually run. Empty is worth saying loudly: a
        // worker with no harness accepts peers and then fails every task.
        if self.providers.is_empty() {
            spans.push(Span::styled(
                "no agent CLI found",
                Style::default().fg(Color::Red),
            ));
        } else {
            spans.push(Span::styled(
                self.providers
                    .iter()
                    .map(|provider| provider.as_str())
                    .collect::<Vec<_>>()
                    .join(", "),
                Style::default().fg(self.theme.primary),
            ));
        }

        // Clipped from the left: the tail of a path identifies a repo, the head
        // is usually a home directory shared by every candidate.
        spans.push(Span::styled("  in ", dim));
        spans.push(Span::styled(
            crate::ui::util::clip_left(&self.primary_workspace, 44),
            Style::default().fg(Color::White),
        ));

        let peers = self.masters.len();
        spans.push(Span::styled(
            format!(
                "  {peers} approved peer{}",
                if peers == 1 { "" } else { "s" }
            ),
            if peers == 0 {
                Style::default().fg(Color::Yellow)
            } else {
                dim
            },
        ));

        if let Some(endpoint) = &self.endpoint {
            spans.push(Span::styled(format!("  via {endpoint}"), dim));
        }
        f.render_widget(Paragraph::new(Line::from(spans)), area);
    }

    /// The first header line: what this process is, and its address.
    fn draw_header_identity(&self, f: &mut Frame, area: Rect) {
        let mut spans = vec![
            Span::styled(
                "WORKER",
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
                "  no host-link identity",
                Style::default().fg(Color::Yellow),
            ));
        }
        f.render_widget(Paragraph::new(Line::from(spans)), area);
    }

    /// The tab bar.
    fn draw_tabs(&mut self, f: &mut Frame, area: Rect) {
        let mut spans = Vec::new();
        let mut ranges = Vec::new();
        let mut x = area.x;
        for (i, name) in TABS.iter().enumerate() {
            // In headless mode the first tab is the log, not a session list;
            // labelling it "Sessions" would promise something that never appears.
            let name = if i == TAB_SESSIONS && self.is_headless() {
                "Log"
            } else {
                name
            };
            let label = format!(" {} {name} ", i + 1);
            let style = if i == self.tab {
                Style::default()
                    .fg(Color::Black)
                    .bg(Color::Cyan)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(Color::DarkGray)
            };
            let end = x
                .saturating_add(label.chars().count() as u16)
                .saturating_sub(1);
            ranges.push((x, end));
            x = end.saturating_add(1);
            spans.push(Span::styled(label, style));
        }
        self.hit_tabs = (area.y, ranges);
        f.render_widget(Paragraph::new(Line::from(spans)), area);
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

        let block = self.panel(format!("Agents · {}", rows.len()), true);
        let inner = block.inner(area);
        f.render_widget(block, area);

        let mut lines: Vec<Line> = Vec::new();
        if rows.is_empty() {
            lines.push(dim("No agents running."));
            lines.push(dim(""));
            if self.providers.is_empty() {
                // A missing binary is a different problem from an empty list and
                // must not read the same.
                lines.push(Line::from(Span::styled(
                    "No coding agents on PATH.",
                    Style::default().fg(Color::Yellow),
                )));
            } else {
                lines.push(dim("Master tasks open agent lanes here."));
            }
        } else {
            let visible = inner.height as usize;
            let start = crate::ui::selection::viewport_start(selected, rows.len(), visible);
            self.hit_rows = Some((inner, start));
            for (i, row) in rows.iter().enumerate().skip(start).take(visible) {
                lines.push(session_line(row, i == selected, self.now(), &self.theme));
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
fn session_line(
    row: &SessionRow,
    selected: bool,
    now: i64,
    theme: &crate::ui::theme::Theme,
) -> Line<'static> {
    let idle = row.idle_ms(now);
    // A running session that has said nothing for a while is the signal an
    // operator is looking for, so it is called out rather than left to be
    // inferred from a timestamp.
    let quiet = if row.state.is_running() && idle >= 10_000 {
        format!(" · quiet {}s", idle / 1_000)
    } else {
        String::new()
    };
    // A harness stopped on its own permission prompt takes the row over. This
    // pane is often the only thing watching an unattended worker, and that state
    // looks exactly like a slow turn from every other signal here — same glyph,
    // same colour, same silence — right up until the task times out.
    let waiting = row
        .attention
        .as_ref()
        .filter(|_| row.state.is_running())
        .map(|cue| format!(" · {ATTENTION_GLYPH} {}", cue.label(now)));
    let mut style = Style::default().fg(state_color(row.state));
    if waiting.is_some() {
        style = Style::default()
            .fg(theme.attention)
            .add_modifier(Modifier::BOLD);
        if theme.attention_blink {
            style = style.add_modifier(Modifier::SLOW_BLINK);
        }
    }
    if selected {
        style = style.add_modifier(Modifier::REVERSED);
    }
    let glyph = match &waiting {
        Some(_) => ATTENTION_GLYPH.to_string(),
        None => row.state.glyph().to_string(),
    };
    crate::ui::agent_lane::line(glyph, row.label.clone(), waiting.unwrap_or(quiet), style)
}
