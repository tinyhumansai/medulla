//! Status-line rendering: connection state, backend host, version notice, and
//! process resource indicators.

use medulla::config::ResourceDisplay;
use ratatui::layout::{Alignment, Constraint, Direction, Layout};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line as TLine, Span};
use ratatui::widgets::Paragraph;
use ratatui::Frame;
use unicode_width::UnicodeWidthStr;

use crate::ui::app::types::App;

impl App {
    /// Draw the bottom status line: the connection dot and backend host, the
    /// update badge, and the right-aligned status text.
    ///
    /// No product name — the login screen already carries the wordmark, and a
    /// status bar that opens by telling you which program you are running spends
    /// its first columns on the one thing you cannot be unsure of.
    pub(super) fn draw_status_line(&mut self, f: &mut Frame, area: ratatui::layout::Rect) {
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
        // Operational messages must remain readable when resource indicators
        // are enabled. Reserve up to a third of the row for a non-empty status;
        // an idle status yields the whole row back to the indicators.
        let status_width = UnicodeWidthStr::width(self.status.as_str());
        let reserved_status_width = status_width.min(usize::from(area.width / 3));
        let available_left_width = usize::from(area.width).saturating_sub(reserved_status_width);
        let base_left_width = spans
            .iter()
            .map(|span| UnicodeWidthStr::width(span.content.as_ref()))
            .sum::<usize>();

        let mut resource_config = self.loaded.config.appearance.clone();
        let sample = self.resource_monitor.sample();
        let mut resource_segments = crate::ui::resources::segments(&resource_config, sample);
        let resource_width = resource_segments.join(" · ").width() + 2;
        if base_left_width + resource_width > available_left_width {
            // Bars and values are the widest representations. Retain every enabled
            // metric at narrow widths by degrading them to their compact percentage
            // form before the pane itself has to clip content.
            for display in [
                &mut resource_config.cpu,
                &mut resource_config.ram,
                &mut resource_config.disk_io,
            ] {
                if *display != ResourceDisplay::Percent && *display != ResourceDisplay::Off {
                    *display = ResourceDisplay::Percent;
                }
            }
            resource_segments = crate::ui::resources::segments(&resource_config, sample);
        }
        if !resource_segments.is_empty() {
            spans.push(Span::styled(
                format!("  {}", resource_segments.join(" · ")),
                Style::default().fg(self.theme.accent),
            ));
        }
        let desired_left_width = spans
            .iter()
            .map(|span| UnicodeWidthStr::width(span.content.as_ref()))
            .sum::<usize>();
        let left_width = desired_left_width.min(available_left_width) as u16;
        let halves = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([
                Constraint::Length(left_width),
                Constraint::Min(reserved_status_width as u16),
            ])
            .split(area);
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
}
