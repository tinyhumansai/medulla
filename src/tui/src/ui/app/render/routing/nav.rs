//! Left-hand navigation for the Routing tab.

use ratatui::layout::Rect;
use ratatui::Frame;

use crate::ui::multi_pane;

use super::super::super::types::{App, ROUTING_SUBPAGES};

impl App {
    /// Draw the Routing page selector and focus hints.
    pub(super) fn draw_routing_nav(&self, f: &mut Frame, area: Rect) {
        multi_pane::draw_nav(
            f,
            area,
            self.panel("Routing"),
            &self.theme,
            &ROUTING_SUBPAGES,
            &[("FLEET", 0)],
            self.routing_index,
            self.routing_focused,
        );
    }
}
