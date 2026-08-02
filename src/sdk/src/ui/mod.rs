//! UI-facing data surface shared with the terminal app: [`events`] (the folded
//! event log + `TuiEvent`), [`agents`] lane folding, [`stream`] token/thread
//! derivations, the [`chat_store`], the [`work`] panel over a harness's own
//! todos and sub-agents, the [`git_review`] model behind the session change
//! set, and small [`util`] helpers. Rendering-heavy
//! screens (app, login, composer, theme) and the interactive onboarding screen
//! live in the `medulla-tui` crate, which re-exports these data modules.

pub mod agents;
pub mod chat_store;
pub mod command;
pub mod decisions;
pub mod events;
pub mod fleet;
pub mod git_review;
pub mod harness;
pub mod meters;
#[cfg(test)]
mod meters_tests;
pub mod stream;
pub mod util;
pub mod work;
#[cfg(feature = "workflows")]
pub mod workflows;
pub mod workspaces;
