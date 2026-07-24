//! Data types shared by reusable multi-pane navigation.

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
