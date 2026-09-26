//! The `[statusLine]` config section: which fields a harness row on the Agents
//! rail shows, where each one sits, and how each one is spelled.
//!
//! A harness row is the only place the operator learns what a session *is* —
//! which CLI, whose it is, which checkout, which branch. Different operators
//! need different parts of that: someone running four checkouts of one repo
//! needs the path and nothing else, someone handing sessions back and forth
//! needs the control state above all. Rather than pick for them, every field
//! declares its own [`FieldPlacement`], and the ones with more than one
//! reasonable spelling declare a style alongside it.
//!
//! Three lines is the ceiling, deliberately. The rail is a list beside the
//! surface the operator is actually reading; a row that can grow without bound
//! turns that list into a second transcript.

mod types;

#[cfg(test)]
mod tests;

pub use types::*;
