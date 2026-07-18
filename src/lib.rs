//! medulla: client SDK and ratatui terminal UI for Medulla. The UI is driven
//! through a `Runtime` trait; concrete runtimes (backend HTTP/SSE, core
//! socket, mock) are wired in `main`. The HTTP/SSE client lives in [`client`].

pub mod auth;
pub mod cli;
pub mod client;
pub mod config;
pub mod daemon;
pub mod home;
pub mod memory;
pub mod runtime;
pub mod session_history;
pub mod tinyplace_support;
pub mod ui;
pub mod wrapper;
