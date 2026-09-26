//! The Settings tab: the grouped left-nav plus every subpage it selects.
//!
//! Settings is where the secondary surfaces live. Alongside the settings proper
//! (Usage, Appearance, Subscriptions, Config) it hosts Feedback, the two diagnostic views
//! (Trace, Context), and Account — all of which used to be top-level tabs. The
//! nav groups them so the diagnostic pages read as diagnostics rather than as
//! peers of the everyday settings.
//!
//! Each subpage's body lives in its own submodule; this one owns only the split,
//! the nav, and the dispatch.

use ratatui::layout::Rect;
use ratatui::text::{Line as TLine, Text};
use ratatui::widgets::{Paragraph, Wrap};
use ratatui::Frame;

use crate::ui::multi_pane;

use super::super::types::{
    App, SP_ACCOUNT, SP_APPEARANCE, SP_CONFIG, SP_CONTEXT, SP_FEEDBACK, SP_STATUS_LINE,
    SP_SUBSCRIPTIONS, SP_TRACE, SP_USAGE,
};

mod account;
mod appearance;
mod config;
mod debug;
mod help;
mod nav;
mod status_line;
mod subscriptions;
mod usage;

/// Return the number of terminal rows a rendered, wrapped footer occupies.
///
/// Both pages with a pinned footer render it through this exact `Paragraph`
/// configuration. Asking Ratatui for its line count avoids duplicating its
/// wrapping semantics, notably for words wider than the available pane.
pub(super) fn rendered_height(lines: &[TLine<'_>], width: u16) -> u16 {
    Paragraph::new(Text::from(lines.to_vec()))
        .wrap(Wrap { trim: false })
        .line_count(width)
        .min(usize::from(u16::MAX)) as u16
}

impl App {
    /// Draw the Settings tab: the grouped subpage nav on the left, the active
    /// subpage on the right.
    pub(super) fn draw_settings(&mut self, f: &mut Frame, area: Rect) {
        let (nav, content) = multi_pane::split(area);
        self.note_pane(nav);
        self.note_pane(content);
        self.draw_settings_nav(f, nav);

        match self.settings_index {
            SP_USAGE => self.draw_usage(f, content),
            SP_APPEARANCE => self.draw_appearance(f, content),
            SP_SUBSCRIPTIONS => self.draw_subscriptions(f, content),
            SP_STATUS_LINE => self.draw_status_line_settings(f, content),
            SP_CONFIG => self.draw_config(f, content),
            SP_FEEDBACK => self.draw_feedback(f, content),
            SP_TRACE => self.draw_trace(f, content),
            SP_CONTEXT => self.draw_context(f, content),
            SP_ACCOUNT => self.draw_account(f, content),
            _ => self.draw_help(f, content),
        }
    }
}
