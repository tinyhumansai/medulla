//! The ratatui render surface for [`App`]. This module owns the outer chrome —
//! the [`App::draw`] layout, hints/tabs/status line, the shared [`App::panel`] block
//! builder, and content dispatch — plus the small styling helpers ([`color`],
//! [`styled_to_tline`], [`event_color`], [`chat_lines`], [`App::event_line`])
//! reused by the per-tab submodules. Each tab's body lives in a sibling module.

use ratatui::layout::{Alignment, Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line as TLine, Span};
use ratatui::widgets::{Block, Paragraph, Wrap};
use ratatui::Frame;

use crate::ui::agents::Line as StyledLine;
use crate::ui::chat::tool_call;
use crate::ui::events::{describe_event, EventEnvelope, TuiEvent};
use crate::ui::util::{clip, clock, wrap};

use super::types::{App, TABS};

mod agents;
mod decisions;
mod harness_modals;
mod memory;
mod overview;
mod points;
mod prompt;
mod routing;
mod selection;
mod settings;
mod template_modal;
#[cfg(feature = "workflows")]
pub(super) mod workflows;

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

/// Render the assembled tool calls, newest last, and clear them.
fn flush_calls(pending: &mut Vec<(i64, PendingCall)>, cols: usize, out: &mut Vec<StyledLine>) {
    for (_, call) in pending.drain(..) {
        // A nameless call is not worth a line. Some providers never send
        // `tool_call_start`, so a whole inference streams as anonymous argument
        // fragments — rendering those gave a column of identical "calling a
        // tool" rows that said nothing the spinner did not. They are replaced
        // by the named list when the inference closes; until then the spinner
        // is the liveness signal.
        if call.name.trim().is_empty() {
            continue;
        }
        // A one-line summary, not the payload: the arguments are frequently
        // kilobytes of JSON, and a transcript that reproduces them whole buries
        // the answer the user is reading for.
        //
        // Streamed arguments are parsed leniently — a call still in flight has
        // a half-written object, which is normal rather than an error, and
        // `summarize` degrades to the verb alone.
        let args = serde_json::from_str::<serde_json::Value>(call.args.trim())
            .unwrap_or(serde_json::Value::Null);
        let text = format!("⏺ {}", tool_call::summarize(&call.name, &args));
        let text = truncate(&text, cols.saturating_sub(2));
        out.push(StyledLine {
            text,
            color: Some("magenta".into()),
            dim: true,
        });
    }
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
        // `InferenceEnd` is excluded because it *supersedes* the streamed
        // assembly rather than following it: flushing here would render every
        // call twice, once from the deltas and once from the authoritative list.
        if !matches!(
            env.event,
            TuiEvent::ToolCallDelta { .. }
                | TuiEvent::Unknown { .. }
                | TuiEvent::InferenceEnd { .. }
        ) {
            flush_calls(&mut pending, cols, &mut out);
        }
        match &env.event {
            // The name arrives once, ahead of its argument fragments.
            // `inference_end` carries the tool calls the model actually made,
            // each with its name and complete arguments. On the backend runtime
            // it arrives untyped — `EventKind` models no variant for it — so it
            // is read from the raw payload here as well as from the typed event
            // below. Either way the named list supersedes the streamed
            // fragments, which are the same calls seen in pieces and often
            // without names.
            TuiEvent::Unknown { kind, data } if kind == "inference_end" => {
                let calls = tool_call::calls_from_payload(data);
                if calls.is_empty() {
                    flush_calls(&mut pending, cols, &mut out);
                } else {
                    pending.clear();
                    for (name, args) in calls {
                        let text = format!("⏺ {}", tool_call::summarize(&name, &args));
                        out.push(StyledLine {
                            text: truncate(&text, cols.saturating_sub(2)),
                            color: Some("magenta".into()),
                            dim: true,
                        });
                    }
                }
            }
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
            // The close of an inference carries the tool calls it actually
            // made, each with its name and complete arguments. The streamed
            // deltas are the same calls seen in pieces — and lossily, since a
            // provider may omit `tool_call_start` and leave the name blank — so
            // the authoritative list replaces them outright.
            TuiEvent::InferenceEnd { tool_calls, .. } => {
                match tool_calls.as_ref().filter(|calls| !calls.is_empty()) {
                    Some(calls) => {
                        pending.clear();
                        for call in calls {
                            let text =
                                format!("⏺ {}", tool_call::summarize(&call.name, &call.args));
                            out.push(StyledLine {
                                text: truncate(&text, cols.saturating_sub(2)),
                                color: Some("magenta".into()),
                                dim: true,
                            });
                        }
                    }
                    // No tool calls reported: whatever the deltas assembled is
                    // all there is, so let it stand.
                    None => flush_calls(&mut pending, cols, &mut out),
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
    /// Draw the whole screen: the shortcut hints, tabs, the active tab's
    /// content, the composer/prompt/resume overlay when applicable, and the
    /// identity/status line.
    ///
    /// The hints ride at the top and the backend/status line at the bottom: the
    /// keys are what a new operator reads, and pinning them under the cursor's
    /// resting place — the tab strip — puts them where the eye already is, while
    /// "which backend am I on, and what is it doing" is a glance-down check.
    pub fn draw(&mut self, f: &mut Frame) {
        self.area = f.area();
        // The harness pane is resolved during the draw and read by the *next*
        // key press, so it has to be cleared here rather than left over: a
        // `Ctrl-]` on the Settings tab must not attach to whatever the Agents
        // tab was showing several frames ago. `draw_agents_pane` fills it back
        // in when it resolves a session.
        self.harness_pane_session = None;
        // Same reasoning as above: a stale rect would route the wheel into a
        // terminal that is no longer on screen.
        self.hit_harness = None;
        // Focus follows the pane, not the other way round. `agents_selection`
        // (called only while drawing the Agents tab) is what notices the cursor
        // moving off the attached session; it has nothing to say once the
        // operator has left the tab entirely. Without this, `harness_focus`
        // stayed `Attached` after a click elsewhere, and the next keystroke —
        // meant for whatever tab was now on screen — was typed into a harness
        // pane the operator could no longer see.
        if self.harness_focus.attached_to().is_some() && self.tab() != "Agents" {
            self.release_harness();
        }
        // The composer now lives inside the Agents pane, so the only things that
        // still claim a row of their own below the content are the inline prompt
        // and the resume picker.
        let has_prompt = self.prompt.is_some();
        let picking = self.resume_picker.is_some();
        let extra = if has_prompt {
            3
        } else if picking {
            self.extra_height()
        } else {
            0
        };
        let rows = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(1), // shortcut hints
                Constraint::Length(1), // tabs
                Constraint::Min(0),    // content
                Constraint::Length(extra),
                Constraint::Length(1), // backend + status
            ])
            .split(self.area);

        // Each frame re-records where the panes landed; a stale rect from the
        // previous layout would confine a drag to a pane that has moved.
        self.panes.clear();
        for row in [rows[0], rows[1], rows[2], rows[4]] {
            self.note_pane(row);
        }
        self.draw_shortcuts(f, rows[0]);
        self.draw_tabs(f, rows[1]);
        self.draw_content(f, rows[2]);
        if self.decision_open {
            self.draw_decisions(f, rows[2]);
        }
        if self.template_modal {
            self.draw_template_modal(f, rows[2]);
        }
        // Above the content, and above each other in answer order: the picker
        // can open the directory prompt, and the hand-back question is asked
        // over whichever pane the operator is releasing.
        if self.harness_picker.is_some() {
            self.draw_harness_picker(f, rows[2]);
        }
        if self.handback_prompt.is_some() {
            self.draw_handback_prompt(f, rows[2]);
        }
        if has_prompt {
            self.draw_prompt(f, rows[3]);
        } else if picking {
            self.draw_resume(f, rows[3]);
        }
        self.draw_status_line(f, rows[4]);
        // Last: the pointer selection paints over whatever the tabs drew, and
        // reads its text back out of the finished buffer.
        self.paint_selection(f);
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

    /// Draw the bottom status line: the connection dot and backend host, the
    /// update badge, and the right-aligned status text.
    ///
    /// No product name — the wordmark is already on the Overview tab, and a
    /// status bar that opens by telling you which program you are running spends
    /// its first columns on the one thing you cannot be unsure of.
    pub(super) fn draw_status_line(&mut self, f: &mut Frame, area: Rect) {
        let halves = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Percentage(60), Constraint::Percentage(40)])
            .split(area);
        let (dot, dot_color) = self.connection_dot();
        let mut spans = vec![
            Span::styled(format!("{dot} "), Style::default().fg(dot_color)),
            // The backend the session is attached to. Host only — the scheme and
            // path are noise in a one-line status bar, and the host is what
            // distinguishes prod from staging from a local dev server.
            Span::styled(
                medulla::config::display_host(&self.loaded.config.backend.base_url),
                Style::default().add_modifier(Modifier::DIM),
            ),
            Span::raw("  "),
        ];
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
        f.render_widget(
            Paragraph::new(TLine::from(Span::styled(
                self.status.clone(),
                Style::default().add_modifier(Modifier::DIM),
            )))
            .alignment(Alignment::Right),
            halves[1],
        );
    }

    /// The connection glyph and colour for the status line's backend host.
    ///
    /// Read from the runtime's event-stream health, which is the closest thing
    /// to a live socket state the UI can see: the core runtime reports `Live`
    /// once its stream is attached and contiguous, `Resyncing` while connecting,
    /// reconnecting, or recovering a sequence gap, and `Stalled` when the
    /// transport is unavailable.
    ///
    /// Runtimes with no stream to track (the mock and plain-HTTP backends)
    /// report nothing, and get a dim hollow dot rather than a green one — an
    /// unknown connection must not read as a healthy one.
    fn connection_dot(&self) -> (char, Color) {
        match self.runtime.stream_state() {
            Some(medulla::runtime::StreamState::Live) => ('●', Color::Green),
            Some(medulla::runtime::StreamState::Resyncing) => ('◌', Color::Yellow),
            Some(medulla::runtime::StreamState::Stalled) => ('✕', Color::Red),
            None => ('○', Color::DarkGray),
        }
    }

    /// Draw the tab bar and record each tab's column span for click hit-testing.
    pub(super) fn draw_tabs(&mut self, f: &mut Frame, area: Rect) {
        self.hit_tabs.clear();
        self.hit_tabs_row = area.y;
        let mut spans = Vec::new();
        let mut col = area.x;
        let roomy_width = TABS
            .iter()
            .map(|name| name.chars().count() + 3)
            .sum::<usize>();
        let gap = if roomy_width <= area.width as usize {
            " "
        } else {
            ""
        };
        for (i, name) in TABS.iter().enumerate() {
            let label = if gap.is_empty() {
                format!("{name} ")
            } else {
                format!(" {name} ")
            };
            let w = label.chars().count() as u16;
            self.hit_tabs.push((col, col + w - 1));
            let mut style = Style::default();
            if i == self.tab_index {
                style = self.theme.selection();
            }
            spans.push(Span::styled(label, style));
            spans.push(Span::raw(gap));
            col += w + gap.len() as u16;
        }
        f.render_widget(Paragraph::new(TLine::from(spans)), area);
    }

    /// Draw the keyboard-shortcut hint line that heads the screen.
    ///
    /// Only keys that act on the surface in front of you — which is also why it
    /// differs per tab: the Agents steering chords do nothing on Workflows, and
    /// advertising them there teaches keys that are not bound. `^O` and `/help`
    /// are still bound; they are just discoverable elsewhere, and a hint line
    /// long enough to wrap stops being read at all.
    pub(super) fn draw_shortcuts(&mut self, f: &mut Frame, area: Rect) {
        #[cfg(feature = "workflows")]
        let workflows = self.tab() == "Workflows";
        #[cfg(not(feature = "workflows"))]
        let workflows = false;
        // An attached harness has swallowed every other binding, so advertising
        // them would be a lie. One key is live, and it is the one that gets the
        // keyboard back.
        if self.harness_focus.attached_to().is_some() {
            f.render_widget(
                Paragraph::new(TLine::from(Span::styled(
                    format!(
                        "Typing into the harness — every key goes to it · {} releases the keyboard",
                        crate::ui::harness_pane::FOCUS_CHORD_LABEL
                    ),
                    Style::default().add_modifier(Modifier::BOLD),
                )))
                .wrap(Wrap { trim: true }),
                area,
            );
            return;
        }
        let text = if self.tab() == "TokenMaxxxing" && self.tokenmaxxxing_is_production() {
            "Tab views · TokenMaxxxing coming soon"
        } else if self.tab() == "TokenMaxxxing" {
            "Tab views · ↑↓ pages · ⏎ open · Esc menu · 1-3 jump"
        } else if workflows {
            "Tab views · ⏎ open · Esc back · ←→ follow edges · ↑↓ lanes · i inspect · c copilot · x run · d dry-run · r refresh"
        } else {
            "Tab views · Esc/↑↓ rail · ⇧⏎ newline · ⌥X cancel · ⌥A answer · ^N thread · ^↑↓ switch · ^Y copy · ^X abort · ^] harness"
        };
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
        crate::ui::widgets::panel(&self.theme, title, false)
    }

    /// Dispatch content rendering to the active tab's draw method.
    pub(super) fn draw_content(&mut self, f: &mut Frame, area: Rect) {
        match self.tab() {
            "Overview" => self.draw_overview(f, area),
            "Agents" => self.draw_agents(f, area),
            #[cfg(feature = "workflows")]
            "Workflows" => self.draw_workflows_tab(f, area),
            "TokenMaxxxing" => self.draw_points(f, area),
            "Routing" => self.draw_routing(f, area),
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

mod types;
use types::PendingCall;
