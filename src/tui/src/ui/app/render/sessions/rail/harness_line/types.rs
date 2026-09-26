//! Data types used while laying out configurable harness status fields.

use ratatui::style::Style;

/// Per-render state shared by every configured field in a harness line.
pub(in crate::ui::app::render::sessions::rail) struct HarnessLineStyle {
    /// Whether the rail cursor is on this harness.
    pub(in crate::ui::app::render::sessions::rail) active: bool,
    /// Maximum display columns available to each rendered line.
    pub(in crate::ui::app::render::sessions::rail) width: usize,
    /// Whether the harness currently needs operator attention.
    pub(in crate::ui::app::render::sessions::rail) alerting: bool,
    /// State or attention glyph drawn by the state field.
    ///
    /// A string rather than a `char` because the working state can animate:
    /// the glyph is one frame of a spinner, chosen by the caller.
    pub(in crate::ui::app::render::sessions::rail) state_glyph: String,
    /// Style for primary status fields.
    pub(in crate::ui::app::render::sessions::rail) primary: Style,
    /// Style for secondary branch and path fields.
    pub(in crate::ui::app::render::sessions::rail) detail: Style,
}

/// A status-line field, in the fixed order used by the renderer.
///
/// Order is not configurable. Placement answers "which line", which is the
/// useful operator choice without multiplying the settings surface.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum Field {
    /// The run-state glyph.
    State,
    /// The harness provider.
    Harness,
    /// Who holds the session.
    Control,
    /// The name assigned to the harness conversation with `/rename`.
    Thread,
    /// The Git branch of the working directory, or its detached commit.
    Branch,
    /// The linked worktree the working directory belongs to, when it is one.
    Worktree,
    /// The working directory.
    Path,
}
