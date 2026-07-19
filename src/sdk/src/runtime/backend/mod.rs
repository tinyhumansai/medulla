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
//! - `set_async_mode` is a purely local flag; the `/medulla/v1` message endpoint
//!   is always called async (`sync=0`) regardless. It changes nothing
//!   server-side and is kept only so the UI toggle has somewhere to land.
//! - `inspect_context` returns an empty list — the backend does not expose the
//!   context store over HTTP.
//! - Roster / presence / peer-session data is empty — that fleet data arrives
//!   over Socket.IO, which this runtime does not open.
//!
//! Split by responsibility: [`types`] holds the local thread/session state model
//! and the [`BackendRuntime`] handle, [`fold`] folds backend events into that
//! state, [`stream`] wires the per-thread SSE tasks, and [`runtime`] implements
//! the [`Runtime`](crate::runtime::Runtime) trait over a live client. The only
//! public item, [`BackendRuntime`], is re-exported here so callers use
//! `medulla::runtime::backend::BackendRuntime`.

mod fold;
mod runtime;
mod stream;
mod types;

#[cfg(test)]
mod tests;

pub use types::BackendRuntime;
