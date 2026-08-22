//! The Status line settings page model and its persistence glue.

mod logic;
mod types;

#[cfg(test)]
mod tests;

pub(super) use logic::{STATUS_LINE_ROWS, STATUS_LINE_ROW_COUNT};
pub(super) use types::*;
