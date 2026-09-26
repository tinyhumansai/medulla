//! The dummy TokenMaxxxing experience.
//!
//! The sidebar separates season progress, daily bounties, and the full
//! leaderboard. Every value is intentionally local sample data so program
//! rules can be reviewed before persistence and eligibility contracts exist.

use ratatui::layout::Rect;
use ratatui::Frame;

use crate::ui::multi_pane;

use super::super::types::{App, TM_BOUNTIES, TM_LEADERBOARD, TM_OVERVIEW, TOKENMAXXING_SUBPAGES};

mod bounties;
mod coming_soon;
mod leaderboard;
mod overview;
mod types;

impl App {
    /// Whether the configured backend is the public production service.
    pub(super) fn tokenmaxxxing_is_production(&self) -> bool {
        medulla::config::display_host(&self.loaded.config.backend.base_url) == "api.tinyhumans.ai"
    }

    /// Draw the sidebar-driven TokenMaxxxing program concept.
    pub(super) fn draw_points(&mut self, frame: &mut Frame, area: Rect) {
        if self.tokenmaxxxing_is_production() {
            self.draw_tokenmaxxxing_coming_soon(frame, area);
            return;
        }
        let (nav, content) = multi_pane::split(area);
        self.note_pane(nav);
        self.note_pane(content);
        self.hit_nav = multi_pane::draw_nav(
            frame,
            nav,
            self.nav_panel("TokenMaxxxing"),
            &self.theme,
            &TOKENMAXXING_SUBPAGES,
            &[],
            self.tokenmaxxing_index,
            self.tokenmaxxing_focused,
        );
        match self.tokenmaxxing_index {
            TM_OVERVIEW => self.draw_tokenmaxxing_overview(frame, content),
            TM_BOUNTIES => self.draw_tokenmaxxing_bounties(frame, content),
            TM_LEADERBOARD => self.draw_tokenmaxxing_leaderboard(frame, content),
            _ => self.draw_tokenmaxxing_overview(frame, content),
        }
    }
}
