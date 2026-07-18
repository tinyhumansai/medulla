//! medulla-tui: a ratatui terminal UI for Medulla, driven through a `Runtime`
//! trait. This crate is self-contained against that trait; the concrete backend
//! runtime is wired separately.

pub mod agents;
pub mod app;
pub mod backend_runtime;
pub mod chat_store;
pub mod clipboard;
pub mod composer;
pub mod config;
pub mod core_client;
pub mod core_runtime;
pub mod daemon;
pub mod events;
pub mod mock_runtime;
pub mod runtime;
pub mod session_history;
pub mod tinyplace_service;
pub mod tinyplace_support;
pub mod util;
