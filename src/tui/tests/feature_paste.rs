//! Bracketed paste into the main TUI.
//!
//! A paste arrives as one `Event::Paste` rather than as a stream of synthetic
//! key presses, so the newlines inside it are text and not `Enter`. These tests
//! pin that down where it used to go wrong: a multi-line paste submitting itself
//! once per line, and a `/`-prefixed paste running whatever the command peek was
//! highlighting. Everything here is state-level — no real terminal is involved,
//! because the terminal mode itself is set once in `TermGuard::setup`.
//!
//! This is the test-binary root. Shared setup lives in `helpers`; the surfaces
//! that accept a paste are split across submodules pulled in via `#[path]` so no
//! single file exceeds the repo's 500-line ceiling. `#[test]` fns inside these
//! included modules are collected and run as part of this binary.

#[path = "feature_paste/helpers.rs"]
mod helpers;

#[path = "feature_paste/composer.rs"]
mod composer;

#[path = "feature_paste/copilot.rs"]
mod copilot;

#[path = "feature_paste/picker.rs"]
mod picker;

#[path = "feature_paste/attached.rs"]
mod attached;
