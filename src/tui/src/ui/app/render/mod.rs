//! The ratatui render surface for [`App`]. This module owns the outer chrome —
//! the [`App::draw`] layout, hints/tabs/status line, the shared [`App::panel`] block
//! builder, and content dispatch — plus the small styling helpers ([`color`],
//! [`styled_to_tline`], [`event_color`], [`App::event_line`])
//! reused by the per-tab submodules. Each tab's body lives in a sibling module,
//! and [`frame_state`] owns the per-frame reset every draw opens with.

use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line as TLine, Span};
use ratatui::widgets::{Block, Paragraph, Wrap};
use ratatui::Frame;

use crate::ui::agents::Line as StyledLine;
use crate::ui::events::{describe_event, EventEnvelope, TuiEvent};
use crate::ui::util::{clip, clock};
use crate::worker::pty::ATTENTION_GLYPH;

use super::types::{App, Overlay, PaneView, TABS};

mod changes;
mod decisions;
mod feedback;
mod frame_state;
pub(super) mod graph;
mod overview;
mod points;
mod prompt;
mod routing;
mod selection;
mod session_modals;
mod sessions;
mod settings;
mod status_line;
mod subconscious;
#[cfg(test)]
mod subconscious_tests;
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
        // Everything the last frame recorded for the next key press, dropped
        // before this one records its own — see [`frame_state`].
        self.reset_frame_state();
        // The composer now lives inside the Sessions pane, so the only things that
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
        // Painted from the one list of what is in front of the content, back to
        // front — the picker can open the directory prompt, and the hand-back
        // question is asked over whichever pane the operator is releasing. The
        // same list answers who owns the keyboard and the clipboard, so an
        // overlay drawn over a composer can never leave that composer quietly
        // taking input behind it.
        for overlay in self.visible_overlays() {
            match overlay {
                Overlay::Decisions => self.draw_decisions(f, rows[2]),
                Overlay::TemplatePopup => self.draw_template_modal(f, rows[2]),
                Overlay::SessionPicker => self.draw_harness_picker(f, rows[2]),
                Overlay::HandbackPrompt => self.draw_handback_prompt(f, rows[2]),
                Overlay::InlinePrompt => self.draw_prompt(f, rows[3]),
                Overlay::ResumePicker => self.draw_resume(f, rows[3]),
            }
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

    /// Draw the tab bar and record each tab's column span for click hit-testing.
    pub(super) fn draw_tabs(&mut self, f: &mut Frame, area: Rect) {
        self.hit_tabs.clear();
        self.hit_tabs_row = area.y;
        let mut spans = Vec::new();
        let mut col = area.x;
        // A harness waiting on the operator is only visible on the Sessions rail,
        // and an operator reading Workflows or Settings is exactly the person
        // who does not know a pane has stopped. The count rides on the tab so
        // the signal survives leaving the tab that carries it.
        let waiting = self.sessions_waiting();
        // Badges are built *before* the width is measured, because they are part
        // of what has to fit: measuring the bare names and then rendering wider
        // labels overflows the bar on a terminal that was only just wide enough,
        // and the tab that pushed past the edge would be the one shouting for
        // attention.
        let badges: Vec<String> = TABS
            .iter()
            .map(|name| {
                if *name == "Sessions" && waiting > 0 {
                    format!(" {ATTENTION_GLYPH}{waiting}")
                } else {
                    String::new()
                }
            })
            .collect();
        let roomy_width = TABS
            .iter()
            .zip(&badges)
            .map(|(name, badge)| name.chars().count() + badge.chars().count() + 3)
            .sum::<usize>();
        let gap = if roomy_width <= area.width as usize {
            " "
        } else {
            ""
        };
        let compact = TABS
            .iter()
            .zip(&badges)
            .map(|(tab, badge)| tab.chars().count() + badge.chars().count() + 1)
            .sum::<usize>()
            > area.width as usize;
        for (i, name) in TABS.iter().enumerate() {
            // Every compact spelling is an actual destination and stays
            // recognizable beside its page title, keeping the whole ring and
            // its mouse hit boxes inside narrow terminals.
            let name = compact_tab_label(name, compact);
            let badge = &badges[i];
            let label = if gap.is_empty() {
                format!("{name}{badge} ")
            } else {
                format!(" {name}{badge} ")
            };
            let w = label.chars().count() as u16;
            self.hit_tabs.push((col, col + w - 1));
            let mut style = Style::default();
            if i == self.tab_index {
                style = self.theme.selection();
            } else if !badge.is_empty() {
                // Only when the tab is not the one you are on: the selection
                // style is how "you are here" is said, and blinking over it
                // would trade a fact for a nag.
                style = self.theme.pulse(self.theme.attention, self.frame);
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
    /// differs per tab: the Sessions steering chords do nothing on Workflows, and
    /// advertising them there teaches keys that are not bound. A hint line long
    /// enough to wrap stops being read at all, so the rest live in Settings ›
    /// Help.
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
                        "Typing into the session — every key goes to it · {} releases the keyboard",
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
        } else if self.tab() == "Subconscious" {
            // Its own line, and a short one: the placeholder binds nothing, and
            // the default hint below advertises the session steering chords.
            "Tab views · Subconscious coming soon"
        } else if self.tab() == "Changes" {
            "Tab views · ↑↓ files · j/k line · [ ] hunk · b baseline · c comment · C file · e edit · r refresh"
        } else if workflows {
            "Tab views · ⏎ open · Esc back · ←→ follow edges · ↑↓ lanes · i inspect · c copilot · x run · d dry-run · r refresh"
        } else if self.tab() == "Sessions" && self.pane_view == PaneView::Diff {
            // The pane is showing a session's diff, so it binds the Changes
            // keys — read from `pane_view` rather than from `pane_session`,
            // which the content draw has not filled in yet this frame.
            "Tab views · ↑↓ files · j/k line · [ ] hunk · b baseline · c comment · d/Esc harness"
        } else {
            "Tab views · ↑↓ rail · ^T new session · ⏎/^] session · d session diff · k close session · ⌥X cancel · ⌥A answer · ^X abort"
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
            "Sessions" => self.draw_sessions_tab(f, area),
            "Changes" => self.draw_changes(f, area),
            #[cfg(feature = "workflows")]
            "Workflows" => self.draw_workflows_tab(f, area),
            // Not feature-gated: the tab exists in the slim build too, and a
            // placeholder has nothing to draw that the workflow engine provides.
            "Subconscious" => self.draw_subconscious(f, area),
            "TokenMaxxxing" => self.draw_points(f, area),
            "Hosts" => self.draw_routing(f, area),
            "Feedback" => self.draw_feedback(f, area),
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

/// Shorten current tab names when their one-space labels cannot all fit.
fn compact_tab_label(name: &str, compact: bool) -> &str {
    if !compact {
        return name;
    }
    match name {
        "Overview" => "Over",
        "Sessions" => "Sess",
        "Workflows" => "Flows",
        "Subconscious" => "Sub",
        "Changes" => "Diff",
        "Feedback" => "Feed",
        "Settings" => "Set",
        _ => name,
    }
}

#[cfg(test)]
mod tests;
