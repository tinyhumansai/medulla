//! A [`Runtime`] backed by the live Medulla backend HTTP + SSE API.
//!
//! Threads map to backend sessions. Each thread runs its own SSE task that
//! folds the backend's `EventEnvelope`s into the thread's local event log, from
//! which snapshots are rendered. State lives behind an `Arc<Mutex<...>>` and a
//! tokio broadcast channel notifies the UI to re-pull a snapshot after every
//! fold, exactly like [`MockRuntime`](crate::runtime::mock::MockRuntime).
//!
//! Divergences from the mock / TS runtime, all because the backend does not (yet)
//! expose the surface:
//! - `fork` has no backend equivalent — the backend has no fork endpoint. We
//!   open a *fresh* session and copy the parent thread's transcript locally, so
//!   the fork diverges from its parent server-side from the first turn.
//! - async delegation mode is not surfaced. The backend now accepts a per-turn
//!   `?detached=` override on the message endpoint (tri-state: omitted inherits
//!   the session-level and server defaults), and [`MedullaClient::send_message`]
//!   takes it, but nothing sources a value: the `set_async_mode` toggle was
//!   removed in #56. This runtime therefore always passes `None`. The message
//!   endpoint is still always called async (`sync=0`), which is unrelated —
//!   `sync` is transport, `detached` is whether a cycle may answer before its
//!   delegated tasks drain.
//! - `inspect_context` returns an empty list — the backend does not expose the
//!   context store over HTTP.
//! - Presence / peer-session data is empty — that arrives over Socket.IO, which
//!   this runtime does not open. The roster is not streamed either, but it *is*
//!   readable: [`refresh_fleet`](crate::runtime::Runtime::refresh_fleet) pulls
//!   `GET /medulla/v1/roster` and [`fleet`] projects it onto the capacity chain.
//!
//! Split by responsibility: [`types`] holds the local thread/session state model
//! and the [`BackendRuntime`] handle, [`fold`] folds backend events into that
//! state, [`fleet`] projects the connected-worker roster onto the capacity
//! chain, [`stream`] wires the per-thread SSE tasks, [`worker_ops`] adapts hub
//! workers and mutations, and [`runtime`] implements the
//! [`Runtime`](crate::runtime::Runtime) trait over a live client. The only public
//! item, [`BackendRuntime`], is re-exported here so callers use
//! `medulla::runtime::backend::BackendRuntime`.

mod fleet;
mod fold;
mod runtime;
mod stream;
mod types;
mod worker_ops;

#[cfg(test)]
mod tests;

pub use types::BackendRuntime;
