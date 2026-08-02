//! Bracketed paste into the main TUI.
//!
//! A paste arrives as one `Event::Paste` rather than as a stream of synthetic
//! key presses, so the newlines inside it are text and not `Enter`. These tests
//! pin that down where it used to go wrong: a multi-line paste submitting itself
//! once per line, and a `/`-prefixed paste running whatever the command peek was
//! highlighting. Everything here is state-level — no real terminal is involved,
//! because the terminal mode itself is set once in `TermGuard::setup`.
//!
//! This is the test binary's root, in the canonical directory layout cargo
//! offers for a multi-file integration test: `tests/feature_paste/main.rs` with
//! ordinary sibling modules. No `feature_paste.rs` sits beside the directory and
//! no `#[path]` is needed, and each surface that accepts a paste gets a file of
//! its own so none approaches the repo's 500-line ceiling.

mod helpers;

mod attached;
mod composer;
mod copilot;
mod picker;
