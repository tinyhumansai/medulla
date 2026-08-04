//! The Hosts page: the `Host → Agents` tree above a preview of the row the
//! cursor is on.
//!
//! The page renders the topology the advert is a projection of (spec §2.4): the
//! hosts this machine runs — always present, running or not — then every remote
//! host the roster reaches, each with the agents known to be on it. It used to
//! render the worker roster flat and call each row a host, which was true only
//! while a machine advertised one worker; a machine now declares one agent per
//! `harness × workspace`, so that list was agents with the host level collapsed
//! out of it.
//!
//! The split into list + preview is what makes roles assignable: folding every
//! row's capacity, readiness and budgets inline cost two rows apiece — mostly
//! reading "details not captured" — and left nowhere for a toggle list that
//! belongs to *one* agent.

use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::Frame;

use super::super::super::types::App;

mod format;
mod list;
mod preview;

impl App {
    /// Draw the host tree with a preview of the selected row beneath it.
    pub(super) fn draw_hosts(&mut self, f: &mut Frame, area: Rect) {
        let (tree, rows, selected) = self.hosts_view();
        self.host_index = selected;

        // Give the preview at most half the page, and never more than the
        // selected row actually has to say. A short tree on a tall terminal
        // should not push its list into a strip.
        let preview_rows = rows
            .get(selected)
            .map(|row| self.preview_lines(&tree, *row).len() as u16 + 2)
            .unwrap_or(0)
            .min(area.height / 2);
        let [list_area, preview_area] = Layout::vertical([
            Constraint::Min(3),
            Constraint::Length(if rows.is_empty() { 0 } else { preview_rows }),
        ])
        .areas(area);

        self.draw_host_list(f, list_area, &tree, &rows, selected);
        if let Some(row) = rows.get(selected).copied() {
            self.draw_host_preview(f, preview_area, &tree, row);
        }
    }
}
