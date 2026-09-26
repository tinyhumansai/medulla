//! The context-sensitive worker status line and its input hints.

use ratatui::layout::Rect;
use ratatui::style::{Color, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use ratatui::Frame;

use super::super::types::{WorkerApp, TAB_MASTER, TAB_SESSIONS, TAB_WORKSPACES};

impl WorkerApp {
    /// Draw the status line with keys for the active context and mouse state.
    pub(super) fn draw_status(&self, f: &mut Frame, area: Rect) {
        let mouse_hint = if self.mouse_capture {
            "^O select text"
        } else {
            "^O enable mouse"
        };
        let hints = if self.confirm.is_some() {
            "y confirm · any other key cancels".to_string()
        } else {
            match self.tab {
                TAB_SESSIONS if self.is_headless() => format!(
                    "↑↓ scroll · click tabs · y copy address · {mouse_hint} · q quit"
                ),
                TAB_SESSIONS => format!(
                    "↑↓/click watch · K kill · d drop · y copy · {mouse_hint} · q quit"
                ),
                TAB_MASTER => format!(
                    "↑↓/click select · a add · i interact · y copy · {mouse_hint} · q quit"
                ),
                TAB_WORKSPACES => format!(
                    "↑↓/click select · a allow · d remove · {mouse_hint} · q quit"
                ),
                _ => format!(
                    "↑↓/click select · a accept · x decline · B block · r refresh · {mouse_hint} · q quit"
                ),
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
}
