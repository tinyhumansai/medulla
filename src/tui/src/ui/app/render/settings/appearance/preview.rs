//! The Preview tab: the current settings drawn as the interface they produce.
//!
//! Sampling rather than describing, for the same reason the Status line page
//! previews its row: "how Medulla highlights a session waiting for you" is a
//! sentence, and a pulsing yellow row is the answer. Every part of it is
//! produced from the live theme and the live `[appearance]` config, and the two
//! resource blocks go through the same [`segments`](crate::ui::resources::segments)
//! and [`device_lines`](crate::ui::resources::device_lines) the real chrome
//! calls, so the preview cannot drift from what it previews.
//!
//! The readings are fixed samples. A preview that moved with the machine's real
//! load would make two settings impossible to compare, and the question this tab
//! answers is about the format, not the number.

use ratatui::style::{Modifier, Style};
use ratatui::text::{Line as TLine, Span};

use medulla::config::{SidebarGrouping, SidebarSort};

use crate::ui::app::types::App;
use crate::ui::resources::{device_lines, segments, DeviceSnapshot, ResourceSnapshot};

/// The widest the sample sidebar is drawn, matching a comfortable rail.
pub(super) const SAMPLE_WIDTH: usize = 34;

/// A fixed process reading, so the status-line sample says the same thing on
/// every machine.
fn process_sample() -> ResourceSnapshot {
    ResourceSnapshot {
        cpu_fraction: 0.25,
        memory_bytes: 512 * 1024 * 1024,
        total_memory_bytes: 32 * 1024 * 1024 * 1024,
        disk_read_bytes_per_second: 4.0 * 1024.0 * 1024.0,
        disk_write_bytes_per_second: 2.0 * 1024.0 * 1024.0,
        disk_peak_bytes_per_second: 16.0 * 1024.0 * 1024.0,
    }
}

/// A fixed whole-device reading, for the same reason.
fn device_sample() -> DeviceSnapshot {
    DeviceSnapshot {
        cpu_fraction: Some(0.42),
        memory_used_bytes: Some(8 * 1024 * 1024 * 1024),
        memory_total_bytes: Some(32 * 1024 * 1024 * 1024),
        disk_used_bytes: Some(300 * 1024 * 1024 * 1024),
        disk_total_bytes: Some(400 * 1024 * 1024 * 1024),
    }
}

/// One sample agent, with everything the sidebar can section or order it by.
struct SampleAgent {
    name: &'static str,
    harness: &'static str,
    host: &'static str,
    path: &'static str,
    title: &'static str,
    /// Position in declaration order, which is what `created` sorts on.
    created: usize,
    /// Position in last-active order, which is what `recent` sorts on.
    recent: usize,
}

/// Two agents, chosen so that every grouping splits them and every sort
/// reorders them — a sample they all agree on would preview nothing.
const SAMPLE_AGENTS: [SampleAgent; 2] = [
    SampleAgent {
        name: "medulla",
        harness: "claude",
        host: "workstation",
        path: "~/work/medulla",
        title: "Ship the status line",
        created: 0,
        recent: 1,
    },
    SampleAgent {
        name: "backend",
        harness: "codex",
        host: "gpu-box",
        path: "~/work/backend",
        title: "Review the queue retry",
        created: 1,
        recent: 0,
    },
];

impl App {
    /// Build the Preview tab's lines.
    pub(super) fn appearance_preview_lines(&self, width: usize) -> Vec<TLine<'static>> {
        let dim = Style::default().add_modifier(Modifier::DIM);
        let sample_width = SAMPLE_WIDTH.min(width.saturating_sub(4)).max(12);

        let mut lines = Vec::new();
        lines.extend(self.preview_chrome(dim, sample_width));
        lines.extend(self.preview_status_line(dim));
        lines.push(TLine::from(""));
        lines.extend(self.preview_sidebar(dim, sample_width));
        lines
    }

    /// The colour roles and the attention cue, drawn as the rows they colour.
    ///
    /// Each sample is captioned in the rule above it rather than in a column
    /// beside it, so the block is as wide as the sample and no wider — which is
    /// what lets the whole preview sit in half a pane.
    fn preview_chrome(&self, dim: Style, width: usize) -> Vec<TLine<'static>> {
        let border = Style::default().fg(self.theme.dim_border);
        let rule = |left: &str, right: &str, caption: &str| {
            let dashes = width.saturating_sub(caption.chars().count() + 2);
            let lead = dashes / 2;
            TLine::from(Span::styled(
                format!(
                    " {left}{} {caption} {}{right}",
                    "─".repeat(lead),
                    "─".repeat(dashes - lead)
                ),
                border,
            ))
        };
        let pad = |used: usize| " ".repeat(width.saturating_sub(used));

        let selected = "▸ Ship the status line";
        let plain = "  Review the queue retry";
        let waiting = "● Waiting for your answer";

        vec![
            TLine::from(Span::styled(
                "Colors",
                Style::default().fg(self.theme.accent),
            )),
            rule("┌", "┐", "selected row"),
            TLine::from(vec![
                Span::styled(" │", border),
                Span::styled(
                    format!("{selected}{}", pad(selected.chars().count())),
                    self.theme.selection(),
                ),
                Span::styled("│", border),
            ]),
            rule("├", "┤", "accent"),
            TLine::from(vec![
                Span::styled(" │", border),
                Span::raw(plain.to_string()),
                Span::raw(pad(plain.chars().count() + 2)),
                Span::styled("2", Style::default().fg(self.theme.accent)),
                Span::styled(" │", border),
            ]),
            rule(
                "├",
                "┤",
                if self.theme.attention_blink {
                    "attention, pulsing"
                } else {
                    "attention, steady"
                },
            ),
            TLine::from(vec![
                Span::styled(" │", border),
                Span::styled(
                    format!("{waiting}{}", pad(waiting.chars().count())),
                    self.theme.pulse(self.theme.attention, self.frame),
                ),
                Span::styled("│", border),
            ]),
            TLine::from(Span::styled(format!(" └{}┘", "─".repeat(width)), border)),
            TLine::from(Span::styled("", dim)),
        ]
    }

    /// The status line's process indicators, through the real formatter.
    fn preview_status_line(&self, dim: Style) -> Vec<TLine<'static>> {
        let segments = segments(&self.loaded.config.appearance, process_sample());
        let body = if segments.is_empty() {
            "  medulla".to_string()
        } else {
            format!("  medulla · {}", segments.join(" · "))
        };
        vec![
            TLine::from(Span::styled(
                "Status line",
                Style::default().fg(self.theme.accent),
            )),
            TLine::from(body),
            TLine::from(Span::styled(
                if segments.is_empty() {
                    "  no process indicators are switched on"
                } else {
                    "  a fixed sample reading, not this machine's"
                },
                dim,
            )),
        ]
    }

    /// The Agents sidebar, sectioned and ordered as configured, with the
    /// device readings that sit under it.
    fn preview_sidebar(&self, dim: Style, width: usize) -> Vec<TLine<'static>> {
        let appearance = &self.loaded.config.appearance;
        let mut order: Vec<&SampleAgent> = SAMPLE_AGENTS.iter().collect();
        match appearance.sidebar_sort {
            SidebarSort::Created => order.sort_by_key(|agent| agent.created),
            SidebarSort::Recent => order.sort_by_key(|agent| agent.recent),
            SidebarSort::Name => order.sort_by_key(|agent| agent.name),
        }

        let mut lines = vec![TLine::from(Span::styled(
            "Sessions sidebar",
            Style::default().fg(self.theme.accent),
        ))];
        let mut section: Option<&str> = None;
        for agent in order {
            let heading = match appearance.sidebar_grouping {
                SidebarGrouping::Host => Some(agent.host),
                SidebarGrouping::Path => Some(agent.path),
                SidebarGrouping::Harness => Some(agent.harness),
                SidebarGrouping::None => None,
            };
            if heading.is_some() && heading != section {
                section = heading;
                lines.push(TLine::from(Span::styled(
                    format!("  {}", heading.unwrap_or_default()),
                    Style::default().fg(self.theme.accent),
                )));
            }
            lines.push(TLine::from(format!(
                "    {:<8} {}",
                agent.harness, agent.name
            )));
            if appearance.show_session_titles {
                lines.push(TLine::from(Span::styled(
                    format!("      {}", agent.title),
                    dim,
                )));
            }
        }

        let device = device_lines(appearance, device_sample(), width, 3);
        if device.is_empty() {
            lines.push(TLine::from(Span::styled(
                "  no device indicators are switched on",
                dim,
            )));
        } else {
            for line in device {
                lines.push(TLine::from(Span::styled(format!("  {line}"), dim)));
            }
        }
        lines
    }
}
