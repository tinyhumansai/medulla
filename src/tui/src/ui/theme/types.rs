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
    /// Foreground used for cues that require operator attention.
    pub attention: Color,
    /// Whether cues that require operator attention blink.
    pub attention_blink: bool,
    /// Length of one attention pulse in milliseconds — bright for the first
    /// half, dim for the second.
    ///
    /// Held in milliseconds rather than the seconds the operator configures so
    /// [`Theme`] stays [`Eq`]: the pulse is compared for equality all over the
    /// render tests, and a float field would make that comparison partial.
    pub attention_blink_ms: u64,
}
