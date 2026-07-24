//! Tests for the worker TUI screen, split by surface so no file exceeds the
//! repo's 500-line ceiling.
//!
//! Shared fixtures live in [`helpers`]; the rest group by what they pin:
//! [`nav_tests`], the [`destructive_tests`] confirmations, [`contact_tests`],
//! and [`render_tests`].

mod helpers;

mod contact_tests;
mod destructive_tests;
mod nav_tests;
mod render_tests;
