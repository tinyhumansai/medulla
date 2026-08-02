//! Data types for the Status line settings page's editable rows.

/// One editable row on the Status line page.
///
/// Each of a field's three questions — where, when, how spelled — is its own
/// row rather than a second axis of one row: `←/→` then means the same thing
/// everywhere, and the page reads as a list of answers rather than as a grid
/// needing another key to move within.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::ui::app) struct StatusLineRow {
    /// The row's label, as shown on the page.
    pub label: &'static str,
    /// Which field of the config it edits.
    pub field: StatusLineField,
    /// Whether it qualifies the row above it, and so is indented under it.
    pub is_qualifier: bool,
}

/// Which status-line configuration value a row edits.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::ui::app) enum StatusLineField {
    /// Where the run-state glyph sits.
    State,
    /// When the run-state glyph is drawn.
    StateWhen,
    /// Where the harness name sits.
    Harness,
    /// When the harness name is drawn.
    HarnessWhen,
    /// How the harness name is spelled.
    HarnessStyle,
    /// Where the control state sits.
    Control,
    /// When the control state is drawn.
    ControlWhen,
    /// How the control state is spelled.
    ControlStyle,
    /// Where the thread name sits.
    Thread,
    /// When the thread name is drawn.
    ThreadWhen,
    /// Where the Git branch sits.
    Branch,
    /// When the Git branch is drawn.
    BranchWhen,
    /// Where the working directory sits.
    Path,
    /// When the working directory is drawn.
    PathWhen,
    /// How much of the working directory is spelled out.
    PathStyle,
}
