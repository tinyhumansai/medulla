//! Streaming a live harness session's screen to a watching orchestrator.
//!
//! The worker half of the `medulla.screen.v1` protocol, whose diff, fold and
//! codec live in the SDK ([`medulla::tinyplace::screen`]). What is here is the
//! part that cannot be: the emulator lives in this crate, so both the
//! translation out of `vt100` and the timer that samples it do too.
//!
//! - [`convert`] — emulator cells to the wire's grid, the one place the two
//!   vocabularies meet.
//! - [`sampler`] — [`SessionStream`], the pure frame decision, plus the task and
//!   registry that drive it.
//! - [`router`] — the inbound half: who may watch what, and starting or stopping
//!   the stream that answers.
//!
//! The worker is the sender and the orchestrator is a passive observer: nothing
//! here accepts input or resizes a session, so a subscription can only ever
//! read. That is why the module needs no trust gate of its own — though *which*
//! sessions a given peer may watch is a question for the caller that owns the
//! subscribe frame, not for the sampler.

pub mod convert;
pub mod router;
pub mod sampler;

#[cfg(test)]
mod tests;

pub use convert::{wire_color, wire_grid, wire_style};
pub use router::ScreenRouter;
pub use sampler::{
    sample_interval, send_fn, spawn_session_stream, LiveCheck, SessionStream, StreamRegistry,
    StreamSpec,
};
