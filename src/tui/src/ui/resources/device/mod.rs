//! Whole-device CPU, memory, and disk-capacity sampling for the sidebar.
//!
//! Where the sibling process monitor answers "what is Medulla costing this
//! machine?", this module answers "how much room does the machine have left?".
//! Sampling is throttled well below the render cadence so a footer that is
//! redrawn every 90ms does not become a source of load itself, and every
//! reading is optional: a platform that cannot report one metric still renders
//! the others.

mod format;
mod monitor;
mod types;

#[cfg(test)]
mod tests;

pub use format::{device_lines, device_width_hint};
pub use types::DeviceMonitor;
