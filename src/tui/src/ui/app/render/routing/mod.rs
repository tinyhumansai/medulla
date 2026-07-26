//! The Routing tab: grouped navigation over the containment chain — the whole
//! declared fleet, the hosts registered on it, the harnesses (and the
//! credentials they spend), the template catalog — plus host onboarding and the
//! default-host strategy.

use ratatui::layout::Rect;
use ratatui::Frame;

use crate::ui::multi_pane;

use super::super::types::{
    App, RP_ADD_HOST, RP_FLEET, RP_HARNESSES, RP_HOSTS, RP_STRATEGIES, RP_TEMPLATES,
};

mod add_host;
mod fleet;
mod harnesses;
mod hosts;
mod nav;
mod strategies;
mod templates;

impl App {
    /// Draw the Routing nav and active content pane.
    pub(super) fn draw_routing(&mut self, f: &mut Frame, area: Rect) {
        let (nav, content) = multi_pane::split(area);
        self.draw_routing_nav(f, nav);
        match self.routing_index {
            RP_FLEET => self.draw_fleet(f, content),
            RP_HOSTS => self.draw_hosts(f, content),
            RP_HARNESSES => self.draw_harnesses(f, content),
            RP_TEMPLATES => self.draw_templates(f, content),
            RP_ADD_HOST => self.draw_add_host(f, content),
            RP_STRATEGIES => self.draw_strategies(f, content),
            _ => self.draw_fleet(f, content),
        }
    }
}
