//! Data types for the `protocol` module.
#[allow(unused_imports)]
use super::*;
/// A parsed serve→host frame. Only the four serve→host frame kinds are
/// represented (`ready`, `res`, `call`, `event`); host→serve frames (`req`,
/// `ret`, `emit`) are never received. A malformed line yields `None` and is
/// skipped by the caller (serve-protocol §1 "parse-fail is skipped, never fatal").
#[derive(Debug)]
pub(in super::super) enum Inbound {
    /// The handshake banner (serve-protocol §3).
    Ready {
        /// Wire version; the host bails on a mismatch.
        protocol: i64,
        /// The serve build version.
        serve: Option<String>,
        /// The session id serve owns.
        session_id: Option<String>,
        /// Non-null when startup failed; the host treats serve as unavailable.
        error: Option<String>,
    },
    /// A correlated response to a host `req`.
    Res {
        /// The `req` id this answers.
        id: String,
        /// Whether the request succeeded.
        ok: bool,
        /// The success payload (`null` on failure).
        result: Value,
        /// The failure detail, when `ok` is false.
        error: Option<WireError>,
    },
    /// A reverse-RPC port callback (serve→host). Not hosted yet in this
    /// milestone; the driver refuses it `port_unavailable`.
    Call {
        /// The callback id serve minted.
        id: String,
        /// The port name (`inference`, `tools`, …).
        port: String,
    },
    /// An unsolicited event-stream frame (serve-protocol §6).
    Event {
        /// The contiguous per-connection sequence.
        seq: u64,
        /// The wrapped `HarnessEvent` (or serve-level frame) payload.
        event: Value,
    },
}
/// Classify a `ready` frame into the handshake outcome the driver acts on.
pub(in super::super) enum ReadyCheck {
    /// Handshake may proceed; carries the serve version + session id.
    Ok {
        /// The serve build version.
        serve: Option<String>,
        /// The session id serve owns.
        session_id: Option<String>,
    },
    /// A fatal handshake outcome; carries the operator-facing reason.
    Fatal(String),
}
