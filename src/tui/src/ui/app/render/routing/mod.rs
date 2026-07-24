//! The Routing tab: grouped navigation for fleet, credentials, and strategy.

use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::Frame;

use super::super::types::{App, RP_ADD_WORKER, RP_KEYS, RP_STRATEGIES, RP_WORKERS};

mod add_worker;
mod keys;
mod nav;
mod strategies;
mod workers;

const NAV_WIDTH: u16 = 20;

impl App {
    /// Draw the Routing nav and active content pane.
    pub(super) fn draw_routing(&mut self, f: &mut Frame, area: Rect) {
        let cols = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Length(NAV_WIDTH), Constraint::Min(0)])
            .split(area);
        self.draw_routing_nav(f, cols[0]);
        match self.routing_index {
            RP_WORKERS => self.draw_workers(f, cols[1]),
            RP_ADD_WORKER => self.draw_add_worker(f, cols[1]),
            RP_KEYS => self.draw_manage_keys(f, cols[1]),
            RP_STRATEGIES => self.draw_strategies(f, cols[1]),
            _ => self.draw_workers(f, cols[1]),
        }
    }
}
