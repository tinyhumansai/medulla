//! The headless `medulla daemon`: offer this machine's local coding-agent CLIs
//! (Claude Code / Codex / OpenCode) as an addressable tiny.place agent over
//! Signal end-to-end encrypted DMs, speaking both plain-text prompts and the
//! `medulla-tinyplace/1` task protocol an orchestrator delegates with.
//!
//! Layout:
//! - [`mappers`] — JSONL transcript → semantic-event line mappers.
//! - [`providers`] — provider detection + one-shot headless task execution.
//! - [`capabilities`] — the on-demand capability probe.
//! - [`transport`] — encrypted Signal DM send/receive + pre-key publishing.
//! - [`types`] — the daemon data model ([`DaemonConfig`], [`DaemonRuntime`], and
//!   the callback aliases).
//! - [`runtime`] + [`task_loop`] — [`DaemonRuntime`], the provider-agnostic task
//!   state machine, split into lifecycle/dispatch and frame/task orchestration.
//! - [`status`] — semantic-event → status-line derivation ([`status_detail`]).
//! - [`flags`] + [`entry`] — CLI flag parsing and the entry ([`run_daemon`]) that
//!   wires the SDK transport in.
//!
//! The interactive PTY wrapper/bridge (node-pty equivalent), the machine bus, the
//! terminal-envelope writer, and the opencode SSE server are intentionally out of
//! scope here — the interactive wrapper lands separately.

pub mod capabilities;
pub mod dir_context;
pub mod mappers;
pub mod providers;
pub mod transport;

mod entry;
mod flags;
mod runtime;
mod status;
mod task_loop;
mod types;

#[cfg(test)]
mod tests;

pub use entry::run_daemon;
pub use status::status_detail;
pub use types::{DaemonConfig, DaemonRuntime, LogFn, NowFn, SendFn};
