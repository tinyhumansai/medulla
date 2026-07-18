//! The terminal UI: the [`app`] state machine the event loop drives, the
//! [`composer`] text input, [`events`] (the folded event log + `TuiEvent`),
//! [`agents`] lane folding, the [`chat_store`], and small [`clipboard`] and
//! [`util`] helpers.

/// The medulla wordmark, rendered on the login screen and the Overview tab.
pub const LOGO: [&str; 3] = ["      ▌  ▜ ▜   ", "▛▛▌█▌▛▌▌▌▐ ▐ ▀▌", "▌▌▌▙▖▙▌▙▌▐▖▐▖█▌"];

pub mod agents;
pub mod app;
pub mod chat_store;
pub mod clipboard;
pub mod composer;
pub mod events;
pub mod login;
pub mod onboarding;
pub mod theme;
pub mod util;
