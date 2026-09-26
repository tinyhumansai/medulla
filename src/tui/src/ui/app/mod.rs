//! The interactive TUI: app state, key/mouse handling, slash commands, and the
//! ratatui render for every tab. A port of the Ink `App.tsx` behavior.
//!
//! The screen is one [`App`] struct whose behaviour is partitioned across sibling
//! submodules that each add an `impl App` block: [`types`] holds the data model,
//! [`state`] construction and accessors, [`input`] event/mouse routing, [`keys`]
//! the keyboard dispatcher, [`commands`] slash-command and steering execution,
//! [`feedback`] the feedback-board subpage's actions and setters,
//! [`settings_edit`] the Config subpage's editable settings, [`account`] the
//! logout action, and [`render`] the ratatui draw for each view. Public items
//! are re-exported here so callers use `crate::ui::app::*`.

mod account;
mod appearance;
mod changes;
mod commands;
mod decisions;
mod feedback;
mod harness_workspace;
#[cfg(test)]
mod harness_workspace_tests;
mod input;
mod keys;
mod overlays;
#[cfg(test)]
mod overlays_tests;
mod rail;
mod remote_hosts;
mod remote_sessions;
#[cfg(test)]
mod remote_sessions_tests;
mod render;
mod session_control;
mod settings_edit;
mod state;
mod status_line;
mod subscriptions;
mod types;
#[cfg(feature = "workflows")]
mod workflows;

#[cfg(test)]
mod tests;

pub use crate::ui::util::SPINNER;
pub use types::{App, Cmd, SETTINGS_SUBPAGES, TABS};
// The pane keys its threads by workflow id with one sentinel for the workflow
// that does not exist yet; the dispatch that runs a turn needs the same mapping
// to find that thread's saved transcript.
#[cfg(feature = "workflows")]
pub use workflows::copilot_thread_of;
