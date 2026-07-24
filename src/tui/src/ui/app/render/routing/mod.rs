//! The Routing tab: grouped navigation for fleet, credentials, and strategy.

use ratatui::layout::Rect;
use ratatui::Frame;

use crate::ui::multi_pane;

use super::super::types::{App, RP_ADD_WORKER, RP_KEYS, RP_STRATEGIES, RP_WORKERS};

mod add_worker;
mod keys;
mod nav;
mod strategies;
mod workers;

impl App {
    /// Draw the Routing nav and active content pane.
    pub(super) fn draw_routing(&mut self, f: &mut Frame, area: Rect) {
        let (nav, content) = multi_pane::split(area);
        self.draw_routing_nav(f, nav);
        match self.routing_index {
            RP_WORKERS => self.draw_workers(f, content),
            RP_ADD_WORKER => self.draw_add_worker(f, content),
            RP_KEYS => self.draw_manage_keys(f, content),
            RP_STRATEGIES => self.draw_strategies(f, content),
            _ => self.draw_workers(f, content),
        }
    }
}
