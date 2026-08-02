//! Rendering-free review state for a session-scoped Git change set.
//!
//! The Changes tab in the terminal app reads commits, files, and patches from
//! Git; this module owns the parts of the review that have nothing to do with
//! either subprocesses or ratatui — where a patch's hunks begin and end, which
//! part of the repository a path's change currently lives in, and the review
//! comments an operator binds to a file, a hunk, or a single patch line.
//!
//! Keeping that model here means a second surface (a web review pane, an MCP
//! tool, a headless report) can reuse the same anchors and comment semantics
//! without depending on the terminal crate.

mod comments;
mod hunks;
mod types;

#[cfg(test)]
mod tests;

pub use comments::ReviewComments;
pub use hunks::{hunks, next_hunk, previous_hunk};
pub use types::{origin_label, ChangeOrigin, CommentAnchor, Hunk, ReviewComment};
