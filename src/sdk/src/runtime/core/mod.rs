//! A [`Runtime`] that attaches to a long-lived `medulla-serve` process over a
//! unix domain socket and mirrors its agent-harness session — the third runtime
//! flavor alongside [`backend`](crate::runtime::backend) (HTTP/SSE) and
//! [`mock`](crate::runtime::mock) (scripted).
//!
//! serve wraps `createAgentHarness` (medulla-v1) and speaks the newline-delimited
//! JSON `medulla-serve` protocol (plan §2.2): a `ready`→`hello` handshake with a
//! protocol-version check, `req`/`res` request flow (`instruct`,
//! `answer_question`, `cancel_task`, `stop`, `subscribe`), and an unsolicited
//! `event` stream this runtime folds into a [`RuntimeSnapshot`](crate::runtime::RuntimeSnapshot).
//! This milestone is **attach-only**: it connects to an existing socket and never
//! spawns or supervises the Node child — process supervision arrives with the
//! separately distributed serve artifact (plan §2.1.5). Reverse-RPC port hosting
//! (serve→host `call` frames) is likewise a later milestone; inbound calls are
//! refused `port_unavailable` so serve never hangs.
//!
//! Split by responsibility: [`types`] holds the shared state model, the
//! connection lifecycle, and the driver command vocabulary; [`protocol`] the
//! NDJSON frame grammar and the event fold; [`client`] the async connection
//! driver and the [`CoreRuntime`] handle; and [`runtime_impl`] the
//! [`Runtime`](crate::runtime::Runtime) trait surface. Unix-only, so the whole
//! module is gated behind `cfg(unix)` by its parent.
//!
//! [`Runtime`]: crate::runtime::Runtime

mod client;
mod protocol;
mod runtime_impl;
mod types;

#[cfg(test)]
mod stub_server;
#[cfg(test)]
mod tests;

pub use client::CoreRuntime;
