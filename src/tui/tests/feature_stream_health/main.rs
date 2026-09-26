//! Feature-level tests for the status line's stream-health indicator: the dot
//! beside the backend host reports the connection whether or not a cycle is
//! running.
//!
//! These need a `Runtime` that actually surfaces a stream state, so a thin
//! `FleetRuntime` wraps `MockRuntime`, delegating everything but `workers()` /
//! `worker_op()` / `stream_state()`.
//!
//! This is the test binary's root, in the canonical directory layout cargo
//! offers for a multi-file integration test: `tests/feature_stream_health/main.rs`
//! with ordinary sibling modules. No `feature_stream_health.rs` sits beside the
//! directory and no `#[path]` is needed. Shared setup lives in `helpers`; the
//! behaviour lives in `header`.

mod helpers;

mod header;
