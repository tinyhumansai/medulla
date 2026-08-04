//! The interactive TUI: app state, key/mouse handling, slash commands, and the
//! ratatui render for every tab. A port of the Ink `App.tsx` behavior.
//!
//! The screen is one [`App`] struct whose behaviour is partitioned across sibling
//! submodules that each add an `impl App` block: [`types`] holds the data model,
//! [`state`] construction and accessors, [`input`] event/mouse routing, [`keys`]
//! the keyboard dispatcher, [`commands`] slash-command and steering execution,
//! [`feedback`] the feedback-board subpage's actions and setters,
//! [`settings_edit`] the Config subpage's editable settings, [`account`] the
//! logout action, [`templates`] the agent-template store's install/reload
//! actions, [`hosts`] the Hosts page's `Host → Agents` tree and its edits,
//! and [`render`] the ratatui draw for each view. Public items
//! are re-exported here so callers use `crate::ui::app::*`.

mod account;
mod agent_control;
#[cfg(test)]
mod agent_control_tests;
mod appearance;
mod changes;
mod commands;
mod credentials;
mod custom_harnesses;
mod decisions;
mod feedback;
mod harness_workspace;
#[cfg(test)]
mod harness_workspace_tests;
mod hosts;
mod input;
mod keys;
mod overlays;
#[cfg(test)]
mod overlays_tests;
mod rail;
mod render;
mod routing_options;
mod session_control;
#[cfg(test)]
mod session_control_tests;
mod session_focus;
#[cfg(test)]
mod session_focus_tests;
mod settings_edit;
mod state;
mod status_line;
mod templates;
mod types;
#[cfg(feature = "workflows")]
mod workflows;
mod workspaces;

#[cfg(test)]
mod tests;

pub use crate::ui::util::SPINNER;
pub use types::{App, Cmd, SETTINGS_SUBPAGES, TABS};
