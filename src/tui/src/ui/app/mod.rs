//! The interactive TUI: app state, key/mouse handling, slash commands, and the
//! ratatui render for every tab. A port of the Ink `App.tsx` behavior.
//!
//! The screen is one [`App`] struct whose behaviour is partitioned across sibling
//! submodules that each add an `impl App` block: [`types`] holds the data model,
//! [`state`] construction and accessors, [`input`] event/mouse routing, [`keys`]
//! the keyboard dispatcher, [`commands`] slash-command and steering execution,
//! and [`render`] the ratatui draw for each tab. Public items are re-exported here
//! so callers use `crate::ui::app::*`.

mod commands;
mod input;
mod keys;
mod render;
mod state;
mod types;

#[cfg(test)]
mod tests;

pub use crate::ui::util::SPINNER;
pub use types::{App, Cmd, SETTINGS_SUBPAGES, TABS};
