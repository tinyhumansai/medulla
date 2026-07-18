//! The terminal UI: the [`app`] state machine the event loop drives, the
//! [`composer`] text input, [`events`] (the folded event log + `TuiEvent`),
//! [`agents`] lane folding, the [`chat_store`], and small [`clipboard`] and
//! [`util`] helpers.

pub mod agents;
pub mod app;
pub mod chat_store;
pub mod clipboard;
pub mod composer;
pub mod events;
pub mod util;
