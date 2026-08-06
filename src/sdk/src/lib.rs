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
pub mod codex_app_server;
pub mod codex_overrides;
pub mod config;
pub mod control_socket;
pub mod core_host;
pub mod daemon;
#[cfg(feature = "workflows")]
pub mod flow_engine;
pub mod harness_contract;
pub mod harness_hooks;
pub mod harness_work;
pub mod history_upload;
pub mod home;
pub mod hub;
pub mod inference_proxy;
pub mod init;
pub mod logging;
/// Medulla's own MCP server, offered to the harnesses it spawns.
///
/// Still gated on `workflows` because the `workflow_*` tool family delegates to
/// [`workflows::ops`]; the `fleet_*` family added beside it depends only on
/// [`control_socket`].
#[cfg(feature = "workflows")]
pub mod mcp;
pub mod onboarding;
pub(crate) mod persistence;
pub mod protocol;
pub mod runtime;
pub mod session_history;
pub mod sessions;
pub mod tokio_tuning;
pub mod ui;
pub mod update;
pub mod worker_profile;
#[cfg(feature = "workflows")]
pub mod workflows;
pub mod wrapper;
