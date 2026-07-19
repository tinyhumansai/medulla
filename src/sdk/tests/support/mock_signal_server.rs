//! A mock tiny.place **Signal server**: the server side of the end-to-end
//! encrypted flows the vendored `tinyplace` SDK drives from the medulla runtime
//! ([`medulla::daemon::transport::SignalTransport`], the wrapper bridge, and the
//! `runtime` mailbox/contact/presence loops).
//!
//! It is a hand-rolled tokio [`TcpListener`] HTTP/1.1 server (one request per
//! connection, `Connection: close`), in the same style as
//! `tests/support/mock_tinyplace.rs` and `tests/support/mock_harness_relay.rs`.
//! The CRYPTO IS LIVE: this server only stores and relays opaque material —
//! published pre-key bundles and encrypted envelopes — and never sees, produces,
//! or verifies plaintext. Real X3DH + double-ratchet runs inside the SDK on both
//! ends; only the transport server is mocked.
//!
//! # Endpoints (auth headers accepted and ignored)
//!
//! Pre-key bundles (registration = publishing a bundle; the identity that lets
//! peers open an encrypted channel):
//! - `PUT  /keys/:id/signed-prekey`  body `{identityKey, signedPreKey}`      → `null`.
//!   Stores the agent's X25519 identity key + signed pre-key. Registration.
//! - `PUT  /keys/:id/prekeys`        body `{identityKey, preKeys:[...]}`      → `null`.
//!   Appends one-time pre-keys to the agent's supply.
//! - `GET  /keys/:id/bundle`         → `KeyBundle`
//!   `{agentId, identityKey, signedPreKey, oneTimePreKey|null, updatedAt}`.
//!   Pops one one-time pre-key per fetch. 404 when the agent has no bundle
//!   (or when a `drop_next_bundle` fault is armed). A `corrupt_next_bundle`
//!   fault tampers the signed-pre-key signature so the initiator rejects it.
//! - `GET  /keys/:id/health`         → `KeyHealth`
//!   `{agentId, oneTimePreKeyCount, lowOneTimePreKeys, updatedAt}`.
//!
//! Mailbox relay (opaque encrypted envelopes, queued per recipient):
//! - `PUT    /messages`              body `MessageEnvelope`                   → the
//!   stored envelope (server assigns `id`). The `body` field is base64
//!   ciphertext; the server records it verbatim for ciphertext assertions.
//! - `GET    /messages?agentId=..`   → `{messages:[MessageEnvelope,...]}`
//!   addressed to `agentId`. Fault knobs reshape this response: `fail_list`
//!   returns 5xx for the next N calls; `duplicate_delivery` returns each queued
//!   envelope twice (same id); `out_of_order` reverses the queue order.
//! - `DELETE /messages/:id?agentId=..` → `null`. Acknowledge (destructive read):
//!   removes the envelope from the queue so it never redelivers.
//!
//! Contacts (pure REST, no ratchet; not enforced for messaging, matching the
//! existing test relays):
//! - `GET  /contacts/requests`       → `{incoming:[{cryptoId,status,direction}],outgoing:[]}`.
//! - `POST /contacts/:id/accept`     → `Contact{requester,addressee,status}` and
//!   records the accepted id, dropping it from the pending set.
//!
//! Presence (pure REST):
//! - `POST /presence/heartbeat`      → `PresenceStatus{cryptoId,online}` (or a
//!   scripted 5xx). Increments a heartbeat counter.
//! - `POST /presence/query`          body `{cryptoIds:[...]}` → `{presence:[{cryptoId,online}]}`.
//!
//! Any other route → 404 `{error:"not found"}`.
//!
//! # State model
//!
//! - `bundles: agentId → {identity_key, signed_pre_key, one_time:[...]}` — the
//!   published key material; `GET bundle` pops one one-time pre-key.
//! - `queue: [envelope,...]` — a single ordered list of opaque envelopes; reads
//!   filter by recipient (`to`), writes assign `m<N>` ids, acks remove by id.
//! - `stored_bodies: [envelope,...]` — an append-only log of every envelope ever
//!   PUT, retained across acks so ciphertext assertions can inspect the full
//!   history (the live `queue` is drained by acks).
//! - `pending_contacts`, `accepted`, `online`, plus request/heartbeat counters.
//!
//! # Fault injection ([`SignalServerControls`])
//!
//! - `drop_next_bundle(n)` — the next `n` `GET bundle` calls 404 (bundle dropped).
//! - `corrupt_next_bundle()` — the next `GET bundle` serves a tampered signature.
//! - `fail_list(n)` — the next `n` `GET /messages` calls return 500.
//! - `set_duplicate_delivery(on)` — `GET /messages` returns each envelope twice.
//! - `set_out_of_order(on)` — `GET /messages` returns the queue reversed.
//! - `heartbeat_status(code)` — the presence heartbeat returns `code`.
//! - `add_pending_contact(id)` / `set_online(ids)` — seed contacts/presence.
//!
//! # Ciphertext guarantee
//!
//! [`MockSignalServer::assert_ciphertext_only`] panics unless, for every stored
//! envelope and every plaintext marker, the marker appears nowhere in the raw
//! envelope JSON and the base64-decoded `body` bytes are either not valid UTF-8
//! or do not contain the marker — proving the server only ever saw ciphertext.
//!
//! # e2e scenario matrix (driven from `tests/e2e_signal.rs`)
//!
//! 1. Two identities register (publish bundles), exchange bundles, round-trip
//!    encrypted DMs; server payloads asserted ciphertext-only; plus presence
//!    heartbeat/query and contact-accept against this server.
//! 2. Owner → daemon task chain: an owner sends a `medulla-tinyplace/1` task
//!    frame; a `DaemonRuntime` receives it over this server, runs it on a mock
//!    harness CLI, and the owner receives ack → status → reply, all encrypted.
//! 3. Same chain against the real `opencode` binary when on PATH (skipped, with a
//!    stderr note, when absent) — a terminal frame (reply or error) must arrive.
//! 4. Wrapper leg: the wrapper bridges a mock-harness session through this server
//!    to an owner; session envelopes arrive encrypted, decrypt to valid v2, and
//!    an inbound control frame reaches the child.
//! 5. Fault matrix: corrupt-bundle self-heal, 5xx-on-list retry, duplicate/
//!    out-of-order delivery, taskKey dedupe, ack drains the queue.
//! 6. medulla-API leg: decrypted frames fold into the expected agents-lane/task
//!    states via `medulla::ui::agents`.

//! # Module layout
//!
//! This entry file is a thin root that wires three sibling submodules (each
//! `#[path]`-included so integration tests can keep pointing `#[path =
//! "support/mock_signal_server.rs"]` at this file):
//! - `state`   — [`ServerState`], [`SignalServerControls`], [`MockSignalServer`].
//! - `routing` — connection handling and per-endpoint dispatch.
//! - `http`    — wire-level request/response + parsing helpers.

#![allow(dead_code, unused_imports)]

#[path = "mock_signal_server_http.rs"]
mod http;
#[path = "mock_signal_server_routing.rs"]
mod routing;
#[path = "mock_signal_server_state.rs"]
mod state;

pub use http::*;
pub use routing::*;
pub use state::*;
