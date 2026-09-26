//! `medulla.remote.v1`: opening, listing and closing sessions on another
//! machine.
//!
//! The control half of a remote session. Its companion is
//! [`screen`](super::screen), which carries the session's screen down and the
//! operator's keystrokes back up — a remote session and a locally-watched one
//! use exactly the same screen protocol, which is why none of that is repeated
//! here.
//!
//! The split is worth stating because it is not arbitrary. Screen messages
//! describe a screen; they have no vocabulary for "start a shell in this
//! directory", and giving them one would mean a viewer that could conjure
//! processes. So the two protocols answer two different questions, share one
//! channel through their version tags, and are parsed by two functions that each
//! decline the other's bodies.
//!
//! Everything here rides channel 0 of the host link — reliable and ordered.
//! Session control is not idempotent: an `Open` delivered twice is two shells,
//! and a lost `Close` is a process nobody reaps.

mod codec;
mod types;

#[cfg(test)]
mod tests;

pub use codec::{encode_remote_message, parse_remote_message};
pub use types::{
    RemoteAttention, RemoteCapabilities, RemoteEnvelope, RemoteHarnessChoice, RemoteMessage,
    RemoteSessionRow, RemoteSessionState, REMOTE_PROTO,
};
