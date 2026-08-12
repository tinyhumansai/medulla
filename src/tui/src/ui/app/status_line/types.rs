//! Data types for the Status line settings page's editable rows.

/// The heading a group of rows is drawn under.
///
/// A row's label answers "which question about this field" — position, shown,
/// spelled — and says nothing about *what the field is*. The group carries that:
/// the field's name, and one line of prose explaining what it puts on a harness
/// row. Without it "Managed / unmanaged" and "State glyph" are names an operator
/// has to already know to use.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::ui::app) struct StatusLineGroup {
    /// The field's name, drawn as the heading.
    pub title: &'static str,
    /// One thin line saying what the field shows on a harness row.
    pub description: &'static str,
}

/// One editable row on the Status line page.
///
/// Each of a field's three questions — where, when, how spelled — is its own
/// row rather than a second axis of one row: `←/→` then means the same thing
/// everywhere, and the page reads as a list of answers rather than as a grid
/// needing another key to move within.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::ui::app) struct StatusLineRow {
    /// The row's label, as shown on the page: the question it answers.
    pub label: &'static str,
    /// Which field of the config it edits.
    pub field: StatusLineField,
    /// The heading this row opens, if it is the first row of its field.
    ///
    /// `None` on the rows that continue a group, so the heading is drawn exactly
    /// once per field and the rows below it read as follow-ups.
    pub group: Option<StatusLineGroup>,
    /// One-line explanation shown in the page's detail footer while this row is
    /// selected, alongside the row's full set of choices.
    pub help: &'static str,
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
    /// Where the linked worktree's name sits.
    Worktree,
    /// When the linked worktree's name is drawn.
    WorktreeWhen,
    /// Where the working directory sits.
    Path,
    /// When the working directory is drawn.
    PathWhen,
    /// How much of the working directory is spelled out.
    PathStyle,
}
