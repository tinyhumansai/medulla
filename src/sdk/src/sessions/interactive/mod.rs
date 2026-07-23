//! The interactive transport: one long-lived harness process fed newline-
//! delimited JSON turns over stdin.
//!
//! This is what makes an [`Unbound`](super::types::SessionClass::Unbound)
//! session a *conversation* rather than a sequence of resumed one-shots. Only
//! `claude` supports it (see
//! [`can_run_interactive`](super::routing::can_run_interactive)); every other
//! provider degrades to the one-shot transport.
//!
//! Three protocol facts this module encodes, each verified against a live CLI
//! and each silently wrong if changed:
//!
//! 1. **Completion is the `result` frame**, not process exit and not a `Stop`
//!    hook. It arrives once per top-level turn on stdout.
//! 2. **Interrupt is in-band** — a `control_request`/`interrupt` on stdin ends
//!    the turn and leaves the process alive.
//! 3. **The interrupt's terminating `result` belongs to the interrupted turn**
//!    and must be drained there, never leaked into the next turn's reader.
//!
//! - [`frames`] — the pure frame↔event fold and the stdin encoders.
//! - [`session`] — the process, the turn loop, and the interrupt protocol.

pub mod frames;
pub mod session;

#[cfg(test)]
mod tests;

pub use frames::{encode_interrupt, encode_user_message, map_stream_frame, StreamEvent};
pub use session::{build_interactive_args, InteractiveSession, InteractiveSpec};
