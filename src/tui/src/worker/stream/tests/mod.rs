//! Unit tests for the worker's half of `medulla.screen.v1`: the emulator-to-wire
//! conversion, the sampler's frame decision, and the dispatch layer that starts
//! and stops a stream.
//!
//! Split by responsibility rather than left as one file, per this repo's
//! per-file line ceiling (`AGENTS.md`):
//!
//! - [`conversion`] — cells and snapshots to the wire's grid and back, including
//!   the whole-snapshot round trip. Pure, no runtime.
//! - [`sampler`] — [`SessionStream`]'s frame decision, alone and folded by a
//!   viewer end to end. Also pure.
//! - [`dispatch`] — the spawned task and [`ScreenRouter`](super::ScreenRouter):
//!   who may watch what, and starting or stopping the stream that answers.
//!   Async and pty-backed.

mod conversion;
mod dispatch;
mod sampler;
