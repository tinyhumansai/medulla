//! Session lifecycle controls for [`App`](super::types::App).
//!
//! The focused child modules own the app's session creation, selection, focus,
//! and close behavior.

mod close;
mod focus;
mod picker;
mod select;
