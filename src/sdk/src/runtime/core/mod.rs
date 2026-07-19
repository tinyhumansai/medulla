//! A [`Runtime`](crate::runtime::Runtime) backed by the core-js orchestration core
//! over its NDJSON Unix socket ([`CoreClient`](crate::runtime::core_client::CoreClient)).
//! This is the second concrete runtime alongside
//! [`BackendRuntime`](crate::runtime::backend) (HTTP/SSE) and
//! [`MockRuntime`](crate::runtime::mock).
//!
//! Threads map to core threads (`thread.list`/`create`/`resume`/`fork`), each with a
//! `thread.subscribe` tap. One connection-wide event receiver funnels every frame;
//! the fold loop routes it by `threadId` and folds it into that thread's event log,
//! applying the §3.3 normalizations in [`map_core_event`]:
//!
//!   - `task_complete` is flat on the wire — rebuilt into the TUI's nested `TaskDigest`,
//!   - the envelope `cycleId` is folded into the lane key (`<cycleId>/t:<taskId>`) so
//!     two cycles delegating the same bare `taskId` never collide (§3.3(2)/§4.4),
//!   - `cancelled` stays distinct from `failed` (handled in `agents.rs`),
//!   - a `task_complete` with no `task_start` still lands a lane (handled in `agents.rs`).
//!
//! A `seq` gap (§3.2) triggers a `snapshot.get` resync: the thread's event log is
//! rebuilt from the durable folded snapshot and a status note is emitted.
//!
//! Split by responsibility: [`types`] holds the in-memory data model
//! ([`CoreRuntime`], `Thread`, `State`), [`events`] the wire → view-model mapping
//! and snapshot fold, [`workers`] the `worker.list` parse, and [`memory`] the
//! persona-memory tool surface. The behavior lives in [`connect`] (the
//! handshake/subscribe/seed constructor), [`fold`] (the live-stream fold loop and
//! stall watchdog), and [`driver`] (the [`Runtime`](crate::runtime::Runtime) impl).
//! All public items are re-exported here so callers use `medulla::runtime::core::*`.

mod connect;
mod driver;
mod events;
mod fold;
mod memory;
mod types;
mod workers;

#[cfg(test)]
mod tests;

pub use events::map_core_event;
pub use memory::MEMORY_TOOLS;
pub use types::CoreRuntime;
