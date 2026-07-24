//! The Settings tab's left-hand navigation: group headings over the flat,
//! selectable subpage list.

use ratatui::layout::Rect;
use ratatui::Frame;

use crate::ui::multi_pane;

use super::super::super::types::{App, SETTINGS_GROUPS, SETTINGS_SUBPAGES};

impl App {
    /// Draw the grouped subpage nav.
    ///
    /// Headings come from [`SETTINGS_GROUPS`] and are interleaved into the flat
    /// [`SETTINGS_SUBPAGES`] list at their start indices, so `settings_index`
    /// stays a plain index into the selectable rows and never has to skip over
    /// non-selectable ones.
    pub(super) fn draw_settings_nav(&mut self, f: &mut Frame, area: Rect) {
        multi_pane::draw_nav(
            f,
            area,
            self.panel("Settings"),
            &self.theme,
            &SETTINGS_SUBPAGES,
            &SETTINGS_GROUPS,
            self.settings_index,
            self.settings_focused,
        );
    }
}
