//! The ratatui render surface for [`App`]. This module owns the outer chrome —
//! the [`App::draw`] layout, header/tabs/footer, the shared [`App::panel`] block
//! builder, and content dispatch — plus the small styling helpers ([`color`],
//! [`styled_to_tline`], [`event_color`], [`chat_lines`], [`App::event_line`])
//! reused by the per-tab submodules. Each tab's body lives in a sibling module.

use ratatui::layout::{Alignment, Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line as TLine, Span};
use ratatui::widgets::{Block, BorderType, Borders, Paragraph, Wrap};
use ratatui::Frame;

use crate::ui::agents::Line as StyledLine;
use crate::ui::events::{describe_event, EventEnvelope, TuiEvent};
use crate::ui::util::{clip, clock, wrap};

use super::types::{App, TABS};

mod agents;
mod chat;
mod feedback;
mod memory;
mod overview;
mod prompt;
mod repo;
mod settings;
mod workers;

/// Map a named color from the agent-lane model to a ratatui [`Color`].
pub(super) fn color(name: &str) -> Color {
    match name {
        "yellow" => Color::Yellow,
        "green" => Color::Green,
        "red" => Color::Red,
        "blue" => Color::Blue,
        "magenta" => Color::Magenta,
        "cyan" => Color::Cyan,
        "cyanBright" => Color::LightCyan,
        "gray" | "grey" => Color::DarkGray,
        "white" => Color::White,
        _ => Color::Reset,
    }
}

/// Convert a styled agent-lane [`StyledLine`] into a ratatui [`TLine`].
pub(super) fn styled_to_tline(line: &StyledLine) -> TLine<'static> {
    let mut style = Style::default();
    if let Some(c) = &line.color {
        style = style.fg(color(c));
    }
    if line.dim {
        style = style.add_modifier(Modifier::DIM);
    }
    let text = if line.text.is_empty() {
        " ".to_string()
    } else {
        line.text.clone()
    };
    TLine::from(Span::styled(text, style))
}

/// The accent color for an event line in the Overview/Trace lists, if any.
pub(super) fn event_color(env: &EventEnvelope) -> Option<&'static str> {
    match &env.event {
        TuiEvent::Error { .. } => Some("red"),
        TuiEvent::TaskStart { .. } | TuiEvent::TaskComplete { .. } | TuiEvent::TaskEvent { .. } => {
            Some("magenta")
        }
        TuiEvent::User { .. } => Some("cyan"),
        TuiEvent::Assistant { .. } => Some("green"),
        TuiEvent::AgentStatus { availability, .. } => Some(if availability == "online" {
            "green"
        } else {
            "red"
        }),
        TuiEvent::InferenceStart { .. } | TuiEvent::InferenceEnd { .. } => Some("blue"),
        _ => None,
    }
}

/// Fold the chat event stream into a wrapped conversational transcript.
/// One tool call being assembled from the stream.
///
/// The name and the arguments arrive as *separate* events — `tool_call_start`
/// carries the name once, the deltas carry argument fragments — so a call can
/// only be rendered after its fragments stop arriving. Held here until the next
/// non-tool event flushes it.
#[derive(Default)]
struct PendingCall {
    name: String,
    args: String,
}

/// Render the assembled tool calls, newest last, and clear them.
fn flush_calls(pending: &mut Vec<(i64, PendingCall)>, cols: usize, out: &mut Vec<StyledLine>) {
    for (_, call) in pending.drain(..) {
        // A one-line summary, not the payload: the arguments are frequently
        // kilobytes of JSON, and a transcript that reproduces them whole buries
        // the answer the user is reading for.
        let name = if call.name.is_empty() {
            "tool".to_string()
        } else {
            call.name.clone()
        };
        let args = compact_args(&call.args);
        let text = if args.is_empty() {
            format!("⏺ {name}")
        } else {
            format!("⏺ {name}({args})")
        };
        let text = truncate(&text, cols.saturating_sub(2));
        out.push(StyledLine {
            text,
            color: Some("magenta".into()),
            dim: true,
        });
    }
}

/// The interesting part of a tool call's JSON arguments, on one line.
///
/// Values only, keys dropped: `{"command":"ls -la"}` reads as `ls -la`, which is
/// what an operator scanning the transcript actually wants. Falls back to the
/// raw text when it is not the object this expects — a half-streamed fragment is
/// normal, not an error.
fn compact_args(raw: &str) -> String {
    let flat: String = raw
        .chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect();
    let flat = flat.split_whitespace().collect::<Vec<_>>().join(" ");
    let Ok(serde_json::Value::Object(map)) = serde_json::from_str::<serde_json::Value>(&flat)
    else {
        return flat;
    };
    map.values()
        .map(|v| match v {
            serde_json::Value::String(s) => s.clone(),
            other => other.to_string(),
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// Clip to `max` display columns, marking that it was clipped.
fn truncate(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        return text.to_string();
    }
    let head: String = text.chars().take(max.saturating_sub(1)).collect();
    format!("{head}…")
}

pub(super) fn chat_lines(events: &[EventEnvelope], width: usize) -> Vec<StyledLine> {
    let cols = width.max(20);
    let mut out = Vec::new();
    // Tool calls are assembled across several events and flushed in stream
    // order, so they appear between the turns they happened between.
    let mut pending: Vec<(i64, PendingCall)> = Vec::new();
    for env in events {
        if !matches!(
            env.event,
            TuiEvent::ToolCallDelta { .. } | TuiEvent::Unknown { .. }
        ) {
            flush_calls(&mut pending, cols, &mut out);
        }
        match &env.event {
            // The name arrives once, ahead of its argument fragments.
            TuiEvent::Unknown { kind, data } if kind == "tool_call_start" => {
                let index = data
                    .get("index")
                    .and_then(serde_json::Value::as_i64)
                    .unwrap_or(0);
                let name = data
                    .get("name")
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or("")
                    .to_string();
                match pending.iter_mut().find(|(i, _)| *i == index) {
                    // A start re-announcing a live index is a *new* call taking
                    // that slot — providers reuse indices across calls. Keeping
                    // the args would render the new call with the old one's
                    // arguments.
                    Some((_, call)) => {
                        call.name = name;
                        call.args.clear();
                    }
                    None => pending.push((
                        index,
                        PendingCall {
                            name,
                            args: String::new(),
                        },
                    )),
                }
            }
            TuiEvent::ToolCallDelta { index, args_delta } => {
                match pending.iter_mut().find(|(i, _)| i == index) {
                    Some((_, call)) => call.args.push_str(args_delta),
                    None => pending.push((
                        *index,
                        PendingCall {
                            name: String::new(),
                            args: args_delta.clone(),
                        },
                    )),
                }
            }
            TuiEvent::User { body } => {
                out.push(StyledLine::default());
                for (i, row) in wrap(body, cols.saturating_sub(2)).into_iter().enumerate() {
                    out.push(StyledLine {
                        text: if i == 0 {
                            format!("❯ {row}")
                        } else {
                            format!("  {row}")
                        },
                        color: Some("cyan".into()),
                        dim: false,
                    });
                }
            }
            TuiEvent::Assistant { body } => {
                for (i, row) in wrap(body, cols.saturating_sub(2)).into_iter().enumerate() {
                    out.push(StyledLine {
                        text: if i == 0 {
                            format!("⏺ {row}")
                        } else {
                            format!("  {row}")
                        },
                        color: Some("green".into()),
                        dim: false,
                    });
                }
            }
            TuiEvent::Error { source, message } => {
                for row in wrap(&format!("{source}: {message}"), cols) {
                    out.push(StyledLine {
                        text: row,
                        color: Some("red".into()),
                        dim: false,
                    });
                }
            }
            _ => {}
        }
    }
    flush_calls(&mut pending, cols, &mut out);
    out
}

impl App {
    /// Draw the whole screen: header, tabs, the active tab's content, the
    /// composer/prompt/resume overlay when applicable, and the footer.
    pub fn draw(&mut self, f: &mut Frame) {
        self.area = f.area();
        let chat = self.tab() == "Chat";
        let has_prompt = self.prompt.is_some();
        let extra = if has_prompt {
            3
        } else if chat {
            self.extra_height()
        } else {
            0
        };
        let rows = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(1), // header
                Constraint::Length(1), // tabs
                Constraint::Min(0),    // content
                Constraint::Length(extra),
                Constraint::Length(1), // footer
            ])
            .split(self.area);

        self.draw_header(f, rows[0]);
        self.draw_tabs(f, rows[1]);
        self.draw_content(f, rows[2]);
        if has_prompt {
            self.draw_prompt(f, rows[3]);
        } else if chat {
            if self.resume_picker.is_some() {
                self.draw_resume(f, rows[3]);
            } else {
                self.draw_composer(f, rows[3]);
            }
        }
        self.draw_footer(f, rows[4]);
    }

    /// The height reserved below the content for the composer or resume picker.
    pub(super) fn extra_height(&self) -> u16 {
        if let Some(p) = &self.resume_picker {
            let cap = ((self.area.height as usize).saturating_sub(9)).max(3);
            (p.chats.len().min(cap) as u16 + 3).min(self.area.height / 2)
        } else {
            let lines = self.draft.text.split('\n').count() as u16;
            lines.max(1) + 2
        }
    }

    /// Draw the top header: the MEDULLA wordmark, the backend host, async/update
    /// badges, and the right-aligned stream-health + status text.
    pub(super) fn draw_header(&mut self, f: &mut Frame, area: Rect) {
        let halves = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Percentage(60), Constraint::Percentage(40)])
            .split(area);
        let mut spans = vec![
            Span::styled(
                "MEDULLA",
                Style::default()
                    .fg(self.theme.primary)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::raw("  "),
            // The backend the session is attached to. Host only — the scheme and
            // path are noise in a one-line header, and the host is what
            // distinguishes prod from staging from a local dev server.
            Span::styled(
                medulla::config::display_host(&self.loaded.config.backend.base_url),
                Style::default().add_modifier(Modifier::DIM),
            ),
            Span::raw("  "),
        ];
        if self.snapshot.async_mode {
            spans.push(Span::styled(
                "⚡ ASYNC ON",
                Style::default()
                    .fg(Color::Magenta)
                    .add_modifier(Modifier::BOLD),
            ));
        } else {
            spans.push(Span::styled(
                "async off",
                Style::default().add_modifier(Modifier::DIM),
            ));
        }
        if let Some(notice) = &self.update_notice {
            spans.push(Span::raw("  "));
            spans.push(Span::styled(
                notice.clone(),
                Style::default()
                    .fg(Color::Yellow)
                    .add_modifier(Modifier::BOLD),
            ));
        }
        f.render_widget(Paragraph::new(TLine::from(spans)), halves[0]);
        // Stream health sits right next to the status when a cycle runs under a
        // runtime that tracks one (the core runtime); otherwise just the status.
        let mut right: Vec<Span> = Vec::new();
        if self.snapshot.running {
            if let Some(st) = self.runtime.stream_state() {
                let c = match st {
                    medulla::runtime::StreamState::Live => Color::Green,
                    medulla::runtime::StreamState::Resyncing => Color::Yellow,
                    medulla::runtime::StreamState::Stalled => Color::Red,
                };
                right.push(Span::styled(
                    format!("{} {}  ", st.glyph(), st.label()),
                    Style::default().fg(c),
                ));
            }
        }
        right.push(Span::styled(
            self.status.clone(),
            Style::default().add_modifier(Modifier::DIM),
        ));
        f.render_widget(
            Paragraph::new(TLine::from(right)).alignment(Alignment::Right),
            halves[1],
        );
    }

    /// Draw the tab bar and record each tab's column span for click hit-testing.
    pub(super) fn draw_tabs(&mut self, f: &mut Frame, area: Rect) {
        self.hit_tabs.clear();
        self.hit_tabs_row = area.y;
        let mut spans = Vec::new();
        let mut col = area.x;
        for (i, name) in TABS.iter().enumerate() {
            let label = format!(" {name} ");
            let w = label.chars().count() as u16;
            self.hit_tabs.push((col, col + w - 1));
            let mut style = Style::default();
            if i == self.tab_index {
                style = self.theme.selection();
            }
            spans.push(Span::styled(label, style));
            spans.push(Span::raw(" "));
            col += w + 1;
        }
        f.render_widget(Paragraph::new(TLine::from(spans)), area);
    }

    /// Draw the footer hint line.
    pub(super) fn draw_footer(&mut self, f: &mut Frame, area: Rect) {
        let text = format!(
            "Tab views · ↑↓ history/nav · ⇧⏎ newline · ^Y copy · ^F fork · ^↑↓ thread · ^X abort · ^O mouse {} · /async {} · /help",
            if self.mouse_capture { "●" } else { "○" },
            if self.snapshot.async_mode { "on" } else { "off" },
        );
        f.render_widget(
            Paragraph::new(TLine::from(Span::styled(
                text,
                Style::default().add_modifier(Modifier::DIM),
            )))
            .wrap(Wrap { trim: true }),
            area,
        );
    }

    /// A rounded, titled panel [`Block`] styled from the active theme.
    pub(super) fn panel<'a>(&self, title: impl Into<String>) -> Block<'a> {
        Block::default()
            .borders(Borders::ALL)
            .border_type(BorderType::Rounded)
            .border_style(Style::default().fg(self.theme.dim_border))
            .title(Span::styled(
                title.into(),
                Style::default()
                    .fg(self.theme.primary)
                    .add_modifier(Modifier::BOLD),
            ))
    }

    /// Dispatch content rendering to the active tab's draw method.
    pub(super) fn draw_content(&mut self, f: &mut Frame, area: Rect) {
        match self.tab() {
            "Overview" => self.draw_overview(f, area),
            "Chat" => self.draw_chat(f, area),
            "Agents" => self.draw_agents(f, area),
            "Repo" => self.draw_repo(f, area),
            "Workers" => self.draw_workers(f, area),
            "Memory" => self.draw_memory(f, area),
            // Trace, Context, and Feedback are Settings subpages, not tabs.
            "Settings" => self.draw_settings(f, area),
            _ => self.draw_overview(f, area),
        }
    }

    /// One formatted event row for the Overview/Trace lists.
    pub(super) fn event_line(
        &self,
        env: &EventEnvelope,
        width: usize,
        selected: bool,
    ) -> TLine<'static> {
        let mut style = Style::default().fg(color(event_color(env).unwrap_or("white")));
        if selected {
            style = self.theme.selection();
        }
        let text = format!(
            "{} {}",
            clock(env.at),
            clip(&describe_event(&env.event), width.saturating_sub(11))
        );
        TLine::from(Span::styled(text, style))
    }
}

#[cfg(test)]
mod tests;
