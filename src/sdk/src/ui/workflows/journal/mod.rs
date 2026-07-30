//! What a workflow has learned, and what it suggests changing, as rows.
//!
//! Notes and proposals want opposite treatments. Notes are many and passive —
//! an operator reads them to understand, not to act — so they render as a list
//! in the inspector. A proposal is singular and actionable, so it renders as a
//! detail block with its verification spelled out, next to the keys that decide
//! it.

mod detail;
mod rows;

pub use detail::proposal_detail;
pub use rows::{actionable, displayed, note_rows, pending, proposal_rows};

#[cfg(test)]
mod tests;
