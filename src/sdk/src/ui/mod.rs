//! UI-facing data surface shared with the terminal app: [`events`] (the folded
//! event log + `TuiEvent`), [`agents`] lane folding, [`stream`] token/thread
//! derivations, the [`chat_store`], and small [`util`] helpers. Rendering-heavy
//! screens (app, login, composer, theme) and the interactive onboarding screen
//! live in the `medulla-tui` crate, which re-exports these data modules.

pub mod agents;
pub mod chat_store;
pub mod command;
pub mod events;
pub mod stream;
pub mod util;
