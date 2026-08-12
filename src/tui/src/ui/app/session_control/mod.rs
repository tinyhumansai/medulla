//! Session lifecycle UI wiring.
//!
//! The focused child modules own handoff state changes, session creation, and
//! closing a local harness. Keeping this module as wiring makes those distinct
//! controls easy to locate without coupling their implementations.

mod close;
mod focus;
mod handoff;
mod picker;
mod select;

#[cfg(test)]
pub(in crate::ui::app) use picker::is_text_input;
