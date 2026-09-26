//! The data model for the interactive TUI screen.
//!
//! This module is deliberately wiring only. Screen state and its supporting
//! model types live in [`model`], the manual-launcher picker and its prompt
//! overlays in [`picker`], and the compact rendered rail hit map in
//! [`rail_hit`]. Keeping those responsibilities separate lets the app's
//! sibling input, rendering, and command modules share the model without
//! turning this directory module into another monolithic source file.

mod model;
mod picker;
mod rail_hit;

pub use model::*;
pub(in crate::ui::app) use picker::*;
pub(in crate::ui::app) use rail_hit::{RailHit, RailHitTarget};
