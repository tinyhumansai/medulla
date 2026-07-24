//! medulla: client SDK for Medulla. The UI-facing surface is driven through a
//! `Runtime` trait; concrete runtimes (backend HTTP/SSE, core socket, mock)
//! live in [`runtime`]. The HTTP/SSE client lives in [`client`]. The terminal
//! app that consumes this crate is the sibling `medulla-tui` crate.

pub mod auth;
pub mod autoreview;
pub mod client;
pub mod clock;
pub mod config;
pub mod contacts;
pub mod daemon;
pub mod harness_contract;
pub mod history_upload;
pub mod home;
pub mod hub;
pub mod init;
pub mod lessons;
pub mod logging;
pub mod memory;
pub mod onboarding;
pub mod runtime;
pub mod session_history;
pub mod sessions;
pub mod tinyplace;
pub mod ui;
pub mod update;
pub mod worker_profile;
pub mod workspace;
pub mod wrapper;
