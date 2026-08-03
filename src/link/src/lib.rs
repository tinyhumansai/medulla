//! `medulla-link` — the transport between a medulla orchestrator and its remote
//! hosts, implementing `docs/host-link-protocol.md`.
//!
//! It replaces the tiny.place mailbox. Instead of Signal-encrypting a frame,
//! pushing it to a hosted relay and polling it back down, two endpoints exchange
//! UDP datagrams carrying **mosh-style state synchronisation** through a backend
//! that forwards bytes it cannot read.
//!
//! ```text
//!   orchestrator ──┐                          ┌── host A
//!    (endpoint)    ├─►  backend forwarder  ◄──┤
//!                  │    (blind, UDP)          └── host B
//!                  └──────────────────────────────┘
//!                      opaque payload, cleartext header
//! ```
//!
//! # The two layers
//!
//! | Layer | Key | Who can read it | Purpose |
//! |---|---|---|---|
//! | Outer header ([`header`]) | forwarder key | backend + endpoints | routing, roaming, replay defence |
//! | Inner payload ([`crypto`], [`packet`]) | pair key | endpoints only | the actual messages |
//!
//! The backend holds forwarder keys and never holds a pair key, so it knows who
//! talks to whom, when and how much — and nothing else.
//!
//! # Why state synchronisation rather than a stream
//!
//! A datagram carries "here is how to get from state *m* to state *n*", and the
//! receiver applies it only when it holds state *m* ([`state`], [`transport`]).
//! Applying the same Instruction twice is a no-op and applying a stale one is
//! refused, so loss, reordering and duplication need no special handling, there
//! is no stream to replay in order, and a peer that was away for a minute gets
//! one diff to *current* rather than a minute of backlog. There is no connection
//! to break: a link that stops working resumes when datagrams flow again, with
//! no reconnect, no handshake and no re-enrollment.
//!
//! # Layout
//!
//! - [`keys`] — node identity, the pair-key codec, and the persisted sequence
//!   reservation that makes nonce reuse impossible across restarts (§3.1).
//! - [`crypto`] — ChaCha20-Poly1305 under the pair key, with the outer header as
//!   AAD (§4).
//! - [`header`] — the 66-byte cleartext header and its forwarder HMAC (§3).
//! - [`packet`] — the plaintext framing and the `Instruction` (§4.1, §4.3).
//! - [`state`] — the `SspState` trait, the append-only `MessageQueue` of channel
//!   0 and the latest-wins `RowGrid` of channel 1 (§4.4, §4.5).
//! - [`transport`] — the per-peer state machine: sent-state history, acks,
//!   RTO, heartbeat and liveness (§4.3, §6). Clock-free and socket-free.
//! - [`Link`] — the tokio driver that gives that state machine a UDP socket.
//!
//! # Where the protocol document leaves room, and what this chose
//!
//! These are interpretation, recorded here because two other implementations
//! code against the same document:
//!
//! - **Key size.** §4 names the pair key as the AEAD key, but the pair key is
//!   128 bits (§7.1) and ChaCha20-Poly1305 takes 256. The key is derived as
//!   `SHA-256("medulla-link/1 pair-key" ‖ pair_key)`; the security level stays
//!   the 128 bits the user carried.
//! - **Send policy.** §6.1 gives `SEND_INTERVAL_MIN`, `SEND_INTERVAL_MAX` and
//!   `RTO` without saying which governs which send. Here: an unsent state change
//!   goes out as soon as `SEND_INTERVAL_MIN` has passed since the last datagram,
//!   an unacknowledged one is retransmitted after `RTO`, and `SEND_INTERVAL_MAX`
//!   bounds how long a bare acknowledgement waits.
//! - **Acknowledgements.** §4.3 has no ack-only datagram. Without one an ack
//!   waits for the next heartbeat, so an idle receiver would stall a sender for
//!   3 seconds. An empty Instruction on channel 0 carrying current ack numbers
//!   serves as one, which is also what a heartbeat is.
//! - **Oversized messages.** §3.2 says to fragment at the state layer, but §4.5
//!   frames whole messages, so a single message larger than one datagram cannot
//!   be sent at all. It is rejected at enqueue
//!   ([`transport::MAX_MESSAGE_BYTES`]) rather than silently wedging the
//!   channel.

pub mod crypto;
pub mod header;
pub mod keys;
pub mod link;
pub mod packet;
pub mod state;
pub mod transport;

pub use link::{Link, LinkConfig, LinkError, LinkHandle, LinkStatus, PeerConfig};
pub use transport::{
    Liveness, Session, SessionConfig, SessionStatus, TransportError, MAX_MESSAGE_BYTES,
};
