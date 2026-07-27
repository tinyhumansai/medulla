//! The Memory tab's first-class pages and selected-entry detail modal.

use ratatui::layout::Rect;
use ratatui::Frame;

use crate::ui::multi_pane;

use super::super::types::{
    App, MEMORY_SUBPAGES, MP_DIRECTIVES, MP_FACETS, MP_MAINTENANCE, MP_OVERVIEW, MP_SEARCH,
};

mod entries;
mod maintenance;
mod overview;
mod popup;

impl App {
    /// Draw the Memory menu, active page, and optional entry-detail modal.
    pub(super) fn draw_memory(&mut self, frame: &mut Frame, area: Rect) {
        let (nav, content) = multi_pane::split(area);
        self.hit_nav = multi_pane::draw_nav(
            frame,
            nav,
            self.panel("Memory"),
            &self.theme,
            &MEMORY_SUBPAGES,
            &[],
            self.memory_subpage_index,
            self.memory_focused,
        );
        match self.memory_subpage_index {
            MP_OVERVIEW => self.draw_memory_overview(frame, content),
            MP_DIRECTIVES => self.draw_memory_directives(frame, content),
            MP_FACETS => self.draw_memory_facets(frame, content),
            MP_SEARCH => self.draw_memory_search(frame, content),
            MP_MAINTENANCE => self.draw_memory_maintenance(frame, content),
            _ => self.draw_memory_overview(frame, content),
        }
        if self.memory_detail_open {
            self.draw_memory_detail_popup(frame, area);
        }
    }
}
