//! A pooled client for `codex app-server` — one long-lived Codex process
//! serving many concurrent threads.
//!
//! # Why this exists
//!
//! The CLI transport forks `codex exec` once per task. Each fork pays the whole
//! startup cost — process spawn, config load, MCP server handshakes, tool
//! discovery — and holds its own copy of that state for as long as the task
//! runs. Ten workflow lanes are ten of everything.
//!
//! `codex app-server` inverts that. It is a JSON-RPC server over stdio that
//! holds one runtime and opens a *thread* per conversation, so ten lanes are ten
//! threads inside one process. The saving is real and it grows with fan-out,
//! which is exactly the shape a workflow has.
//!
//! What it costs is the CLI's rendering: an app-server thread reports structured
//! items rather than the JSONL stream the [`mappers`](crate::daemon::mappers)
//! were written against. This module deliberately does not reimplement that
//! surface — see [`crate::daemon::providers::codex_server`] for the minimal fold
//! it does provide.
//!
//! # Layout
//!
//! - [`types`] — the data model: pool keys, spawn specs, thread options, the
//!   turn outcome, and the error type.
//! - [`jsonrpc`] — the line-framed JSON-RPC message model this speaks.
//! - [`records`] — bounded line framing over the child's stdout.
//! - [`connection`] — one supervised child process: the reader/writer tasks, the
//!   pending-request table, and per-thread notification fan-out.
//! - [`pool`] — the process-sharing seam: connections keyed by everything that
//!   would make two runs need different processes.
//!
//! # What is shared and what is not
//!
//! A pooled connection is shared by every task whose [`AppServerKey`] matches —
//! same binary, same `CODEX_HOME`, same credential-bearing environment. Anything
//! that would change *who the process authenticates as* or *what it can reach*
//! is part of the key, never a per-thread override, because a process cannot
//! hold two answers to those at once.
//!
//! Everything per-task — cwd, model, sandbox, approval policy, the prompt — is a
//! thread or turn parameter, which the protocol scopes correctly. Two threads in
//! one process do not see each other's context.

mod connection;
mod jsonrpc;
mod pool;
mod types;

#[cfg(test)]
mod tests;

pub use connection::{Connection, ThreadSubscription};
pub use jsonrpc::{Message, Notification, RequestId};
pub use pool::{pool, AppServerPool};
pub use types::{
    AppServerError, AppServerKey, AppServerSpec, ApprovalPolicy, SandboxMode, ThreadOptions,
    TurnOutcome, TurnStatus, CLIENT_NAME,
};
