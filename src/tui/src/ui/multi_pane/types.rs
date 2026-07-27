//! Data types shared by reusable multi-pane navigation.

use ratatui::layout::Rect;

/// Where a drawn subpage nav put its clickable page rows.
///
/// Carries the menu's own rectangle as well as the rows, so a click on the
/// content pane beside the menu cannot select whichever page shares its row.
#[derive(Debug, Clone, Default)]
pub(crate) struct NavHits {
    /// The menu's inner area, borders excluded.
    pub(crate) area: Rect,
    /// One `(screen_row, page_index)` per selectable page row.
    pub(crate) rows: Vec<(u16, usize)>,
}

impl NavHits {
    /// The page at `(x, y)`, or `None` when the point misses the menu.
    pub(crate) fn page_at(&self, x: u16, y: u16) -> Option<usize> {
        if !self.area.contains((x, y).into()) {
            return None;
        }
        self.rows
            .iter()
            .find(|(row, _)| *row == y)
            .map(|(_, page)| *page)
    }
}

/// What shared navigation did with one key press.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum NavAction {
    /// The active page changed.
    SelectionChanged,
    /// Focus moved from the menu into the content pane.
    Entered,
    /// Focus moved from the content pane back to the menu.
    Left,
    /// The key belongs to the current mode but changed no state.
    Consumed,
    /// Page-specific or global handling should receive the key.
    Unhandled,
}
