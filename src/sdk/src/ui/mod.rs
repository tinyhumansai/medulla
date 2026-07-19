//! UI-facing data surface shared with the terminal app: [`events`] (the folded
//! event log + `TuiEvent`), [`agents`] lane folding, the [`chat_store`], the
//! [`onboarding`] screen the daemon/wrapper first-run flow drives, and small
//! [`util`] helpers. Rendering-heavy screens (app, login, composer, theme)
//! live in the `medulla-tui` crate, which re-exports these modules.

pub mod agents;
pub mod chat_store;
pub mod events;
pub mod onboarding;
pub mod util;
