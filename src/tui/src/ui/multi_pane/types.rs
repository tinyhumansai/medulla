//! Data types shared by reusable multi-pane navigation.

use ratatui::layout::Rect;
use ratatui::style::Color;

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

/// One row of a navigation sidebar.
///
/// The intermediate form every sidebar in the app is built from, so a fixed page
/// list (Settings, Routing) and a list read from a store (Workflows) render
/// identically rather than approximately. Before this the Workflows rail
/// reimplemented the marker, the selection styling, and the hint line, and had
/// already drifted on two of the three.
#[derive(Debug, Clone)]
pub(crate) struct NavRow<'a> {
    /// The text of the row, without marker, digit, or indent.
    pub(crate) label: &'a str,
    /// The digit that jumps here, when the row is one a digit can reach.
    /// `None` for a heading or a child row, which are not jump targets.
    pub(crate) jump: Option<usize>,
    /// Leading spaces, for rows nested under the one above them.
    pub(crate) indent: usize,
    /// Whether the cursor is on this row.
    pub(crate) selected: bool,
    /// Whether the row is secondary — a heading, or an entry that is disabled
    /// or otherwise not actionable.
    pub(crate) dim: bool,
    /// Optional semantic foreground colour, used by status-bearing rows.
    pub(crate) color: Option<Color>,
    /// Whether a click on the row should select something. Headings are drawn
    /// but inert, which is what keeps a click on one from selecting whichever
    /// entry happens to share its offset.
    pub(crate) selectable: bool,
}

impl<'a> NavRow<'a> {
    /// A plain selectable row carrying `label`.
    pub(crate) fn new(label: &'a str) -> Self {
        Self {
            label,
            jump: None,
            indent: 0,
            selected: false,
            dim: false,
            color: None,
            selectable: true,
        }
    }

    /// A display-only heading.
    pub(crate) fn heading(label: &'a str) -> Self {
        Self {
            label,
            jump: None,
            indent: 0,
            selected: false,
            dim: true,
            color: None,
            selectable: false,
        }
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
