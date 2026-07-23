//! The terminal UI: the [`app`] state machine the event loop drives, the
//! [`composer`] text input, the [`login`] screen, the [`onboarding`] first-run
//! screen, [`theme`] styling, and the [`clipboard`] helper. The data-facing
//! modules ([`events`], [`agents`], [`chat_store`], [`util`]) live in the
//! `medulla` SDK and are re-exported here so `crate::ui::...` paths cover the
//! whole surface.

/// The medulla wordmark, rendered on the login screen and the Overview tab.
pub const LOGO: [&str; 3] = ["      ▌  ▜ ▜   ", "▛▛▌█▌▛▌▌▌▐ ▐ ▀▌", "▌▌▌▙▖▙▌▙▌▐▖▐▖█▌"];

pub use medulla::ui::{agents, chat_store, command, decisions, events, harness, stream, util};

pub mod app;
pub mod clipboard;
pub mod composer;
pub mod login;
pub mod onboarding;
pub mod theme;
pub mod welcome;
