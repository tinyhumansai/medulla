//! Unit + in-crate integration tests for the core (`medulla-serve`) runtime,
//! split by surface: [`connection_tests`] drives a [`StubServer`](super::stub_server::StubServer)
//! over a real unix socket to cover the handshake, reconnect, and steering
//! surface; [`protocol_tests`] covers the pure NDJSON frame grammar; and
//! [`state_tests`] covers the event fold and [`CoreState`](super::types::CoreState)
//! model.
//!
//! Shared async-poll helper used by more than one child module lives here.

use std::time::Duration;

mod connection_tests;
mod headless_tests;
mod protocol_tests;
mod state_tests;

/// Poll `f` up to ~2 s, returning whether it ever held. Keeps the async socket
/// tests deterministic without sleeping a fixed amount.
async fn wait_until<F: Fn() -> bool>(f: F) -> bool {
    for _ in 0..200 {
        if f() {
            return true;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    f()
}
