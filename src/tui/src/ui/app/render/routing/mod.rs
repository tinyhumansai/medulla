//! The Routing tab: the fleet's management surfaces — the hosts registered on
//! it, the harnesses (and the credentials they spend), the directories there are
//! to work in, the template catalog —
//! plus host onboarding and the default-host strategy. The declared tree itself
//! is rendered in the Agents rail, not here, and workflows have a tab of their
//! own — they are work rather than capacity.

use ratatui::layout::Rect;
use ratatui::Frame;

use crate::ui::multi_pane;

use super::super::types::{
    App, RP_ADD_HOST, RP_HARNESSES, RP_HOSTS, RP_STRATEGIES, RP_TEMPLATES, RP_WORKSPACES,
};

mod add_host;
mod harnesses;
mod hosts;
mod nav;
mod strategies;
mod templates;
mod workspaces;

impl App {
    /// Draw the Routing nav and active content pane.
    pub(super) fn draw_routing(&mut self, f: &mut Frame, area: Rect) {
        let (nav, content) = multi_pane::split(area);
        self.draw_routing_nav(f, nav);
        match self.routing_index {
            RP_HOSTS => self.draw_hosts(f, content),
            RP_HARNESSES => self.draw_harnesses(f, content),
            RP_WORKSPACES => self.draw_workspaces(f, content),
            RP_TEMPLATES => self.draw_templates(f, content),
            RP_ADD_HOST => self.draw_add_host(f, content),
            RP_STRATEGIES => self.draw_strategies(f, content),
            _ => self.draw_hosts(f, content),
        }
    }
}
