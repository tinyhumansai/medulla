//! Data types for the `theme` module.
#[allow(unused_imports)]
use super::*;
/// The resolved color roles used across the TUI.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Theme {
    /// Selection/highlight background, brand, panel titles, and primary accents.
    pub primary: Color,
    /// Secondary accent for inline overlays (prompt/resume borders).
    pub accent: Color,
    /// Foreground drawn on top of `primary` for selected rows.
    pub selection_fg: Color,
    /// Dim panel border color.
    pub dim_border: Color,
}
