//! The reusable chat surface: a transcript over a composer.
//!
//! Two surfaces in this app are the same gesture — the orchestrator on the
//! Sessions tab and the copilot on the Workflows tab. Both are "read what happened,
//! then say what you want next", and before this module they were two
//! implementations of it that had already drifted: the copilot's input collapsed
//! a multi-line draft onto one row, drew no caret unless focused, and had no
//! border to say whether it held the keyboard.
//!
//! So the parts that are not about *what* is being discussed live here:
//!
//! - [`composer`] — the bordered, focus-aware input with a real multi-line caret.
//! - [`transcript`] — bottom-anchored scrollback with a clamped scroll offset.
//! - [`tool_call`] — turning a harness tool call into one readable line.
//!
//! What stays with each caller is the thing that genuinely differs: where the
//! lines come from. The orchestrator folds a live event stream
//! ([`super::app::render::chat_lines`]); the copilot renders role-tagged turns.

pub(crate) mod composer;
pub(crate) mod transcript;

pub(crate) mod types;

pub(crate) use composer::{composer_height, draw_composer};
pub(crate) use transcript::draw_transcript;
pub(crate) use types::ComposerChrome;

#[cfg(test)]
mod tests;
