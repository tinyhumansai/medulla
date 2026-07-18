//! Shared e2e test helpers: an in-test mock Medulla backend (HTTP + SSE) and
//! fake provider-CLI scaffolding for the daemon runtime.
//!
//! Every test binary under `tests/` includes this via `mod support;`; not all of
//! them use every helper, so blanket-allow the unused-code lints here rather than
//! sprinkling `#[allow]` at each call site.
#![allow(dead_code)]

pub mod fake_provider;
pub mod mock_backend;

use std::time::{Duration, Instant};

/// Poll `predicate` until it returns true or `timeout` elapses. Panics with
/// `label` on timeout. Uses a short interval to keep e2e wall time small.
pub async fn wait_until<F>(label: &str, timeout: Duration, mut predicate: F)
where
    F: FnMut() -> bool,
{
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        if predicate() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    panic!("timed out waiting for: {label}");
}
