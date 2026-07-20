//! The Settings tab: the grouped left-nav plus every subpage it selects.
//!
//! Settings is where the secondary surfaces live. Alongside the settings proper
//! (Usage, Appearance, Config) it hosts Feedback, the two diagnostic views
//! (Trace, Context), and Account — all of which used to be top-level tabs. The
//! nav groups them so the diagnostic pages read as diagnostics rather than as
//! peers of the everyday settings.
//!
//! Each subpage's body lives in its own submodule; this one owns only the split,
//! the nav, and the dispatch.

use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::Frame;

use super::super::types::{
    App, SP_ACCOUNT, SP_APPEARANCE, SP_CONFIG, SP_CONTEXT, SP_FEEDBACK, SP_TRACE, SP_USAGE,
};

mod account;
mod appearance_usage;
mod config;
mod debug;
mod help;
mod nav;

/// The left-nav width. Wide enough for the indented rows under their headings.
const NAV_WIDTH: u16 = 20;

impl App {
    /// Draw the Settings tab: the grouped subpage nav on the left, the active
    /// subpage on the right.
    pub(super) fn draw_settings(&mut self, f: &mut Frame, area: Rect) {
        let cols = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Length(NAV_WIDTH), Constraint::Min(0)])
            .split(area);

        self.draw_settings_nav(f, cols[0]);

        match self.settings_index {
            SP_USAGE => self.draw_usage(f, cols[1]),
            SP_APPEARANCE => self.draw_appearance(f, cols[1]),
            SP_CONFIG => self.draw_config(f, cols[1]),
            SP_FEEDBACK => self.draw_feedback(f, cols[1]),
            SP_TRACE => self.draw_trace(f, cols[1]),
            SP_CONTEXT => self.draw_context(f, cols[1]),
            SP_ACCOUNT => self.draw_account(f, cols[1]),
            _ => self.draw_help(f, cols[1]),
        }
    }
}
