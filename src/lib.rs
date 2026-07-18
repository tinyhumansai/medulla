//! medulla: client SDK and ratatui terminal UI for Medulla. The UI is driven
//! through a `Runtime` trait; concrete runtimes (backend HTTP/SSE, core
//! socket, mock) are wired in `main`. The HTTP/SSE client lives in [`client`].

pub mod agents;
pub mod app;
pub mod backend_runtime;
pub mod client;
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
