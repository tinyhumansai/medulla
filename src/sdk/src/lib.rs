//! medulla: client SDK for Medulla. The UI-facing surface is driven through a
//! `Runtime` trait; concrete runtimes (backend HTTP/SSE, core socket, mock)
//! live in [`runtime`]. The HTTP/SSE client lives in [`client`]. The terminal
//! app that consumes this crate is the sibling `medulla-tui` crate.

pub mod agents;
pub mod attribution;
pub mod auth;
pub mod bridge;
pub mod client;
pub mod clipboard;
pub mod clock;
pub mod config;
pub mod contacts;
pub mod core_host;
pub mod daemon;
#[cfg(feature = "workflows")]
pub mod flow_engine;
pub mod harness_contract;
pub mod harness_work;
pub mod history_upload;
pub mod home;
pub mod hub;
pub mod inference_proxy;
pub mod init;
pub mod logging;
pub mod onboarding;
pub(crate) mod persistence;
pub mod runtime;
pub mod session_history;
pub mod sessions;
pub mod tinyplace;
pub mod tokio_tuning;
pub mod ui;
pub mod update;
pub mod worker_profile;
#[cfg(feature = "workflows")]
pub mod workflows;
pub mod wrapper;
