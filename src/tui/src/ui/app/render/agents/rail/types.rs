//! Data types owned by the Agents rail.
//!
//! Behaviour lives beside the logic that uses it — [`DeviceFooter`]'s sampling
//! and styling stay in [`device_footer`](super::device_footer); only the shape
//! is declared here.

/// Prepared footer content and the navigation capacity left above it.
pub(super) struct DeviceFooter {
    /// Formatted device readings, already fitted to the rail width.
    pub(super) lines: Vec<String>,
    /// Number of rail lines available for selectable navigation rows.
    pub(super) navigation_capacity: usize,
    /// Total rail height, used to pin the footer to the bottom.
    pub(super) rail_height: usize,
}
