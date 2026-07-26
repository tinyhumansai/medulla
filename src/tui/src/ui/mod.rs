//! The terminal UI: the [`app`] state machine the event loop drives, the
//! [`composer`] text input, the [`login`] screen, the [`onboarding`] first-run
//! screen, and [`theme`] styling. The data-facing modules ([`events`],
//! [`agents`], [`chat_store`], [`util`]) and the [`clipboard`] helper live in
//! the `medulla` SDK and are re-exported here so `crate::ui::...` paths cover
//! the whole surface. The clipboard sits in the SDK because the worker daemon
//! needs it too: handing a worker's address to the operator's terminal is how
//! pairing avoids a manual copy off a remote shell.

/// The medulla wordmark, rendered on the login screen and the Overview tab.
pub const LOGO: [&str; 3] = ["      ▌  ▜ ▜   ", "▛▛▌█▌▛▌▌▌▐ ▐ ▀▌", "▌▌▌▙▖▙▌▙▌▐▖▐▖█▌"];

pub use medulla::clipboard;
pub use medulla::ui::{
    agents, chat_store, command, decisions, events, fleet, harness, meters, stream, util,
};

pub(crate) mod agent_lane;
pub mod app;
pub mod composer;
pub(crate) mod layout;
pub mod login;
pub(crate) mod multi_pane;
pub mod onboarding;
pub(crate) mod selection;
pub mod theme;
pub mod welcome;
pub(crate) mod widgets;
