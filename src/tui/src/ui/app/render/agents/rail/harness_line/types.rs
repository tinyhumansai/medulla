//! Data types used while laying out configurable harness status fields.

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
    /// The Git branch of the working directory.
    Branch,
    /// The working directory.
    Path,
}
