//! `CoreClient` — an async NDJSON RPC client over the core-js Unix socket
//! (§1–§3 of docs/unified-app/04-protocol-contract.md).
//!
//! It owns one `tokio::net::UnixStream`, split into a write half (behind a mutex,
//! used by `request`) and a read half driven by a background task. That task
//! correlates each `{id, ok|error}` response with the `request` awaiting it (via a
//! per-id `oneshot`) and forwards every unsolicited `{"t":"event", ...}` frame to
//! the events channel handed back from [`CoreClient::connect`], so the runtime can
//! fold the stream (§3.2).
//!
//! The module is split by responsibility: [`types`] holds the protocol constants
//! and the plain data surfaces ([`CoreEvent`], [`RpcError`], [`CallError`],
//! [`SeqTracker`]); [`client`] holds the connection logic — the [`CoreClient`]
//! itself, its typed RPC methods, and the background NDJSON read loop. All public
//! items are re-exported here so callers use `medulla::runtime::core_client::*`.

mod client;
mod types;

#[cfg(test)]
mod tests;

pub use client::CoreClient;
pub use types::{CallError, CoreEvent, RpcError, SeqTracker, MAX_FRAME_BYTES, PROTOCOL_VERSION};
