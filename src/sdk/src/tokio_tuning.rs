//! Tokio runtime tuning for any process that may host an agent turn.
//!
//! These are process-level knobs, not SDK behaviour, but they live here rather
//! than in the app crate for one reason: the embedded OpenHuman core is a
//! dependency of *this* crate, so this is the only place that can source the
//! values from it. Keeping the seam here is the same rule that puts every other
//! vendored-crate coupling in the SDK.
//!
//! Both values are re-exported from OpenHuman rather than restated, so the two
//! hosts cannot drift apart. OpenHuman's own runtime docs direct downstream call
//! sites to set both on every multi-thread runtime that can host an agent turn.

/// Worker-thread stack size for a runtime that may host an agent turn.
///
/// tokio defaults worker threads to 2 MiB, which is not enough: the agent
/// harness async chain is deep enough that OpenHuman raises its own
/// `recursion_limit` to 256 for it, and a nested delegation overflows the
/// default stack.
///
/// That failure mode is why this matters more than it looks — it reproduces
/// only under real nesting, so it surfaces in front of a user rather than in a
/// unit test. The same depth already overflows `cargo test`'s 2 MiB test
/// threads, which is why the suite needs `RUST_MIN_STACK=16777216`.
pub const WORKER_STACK_BYTES: usize = openhuman_core::core::runtime::AGENT_WORKER_STACK_BYTES;

/// Upper bound on tokio's blocking-thread pool.
///
/// tokio defaults to 512. Blocking work in this process is bursty and bounded
/// (filesystem, sqlite, process spawn), so the default mostly buys idle
/// footprint. Threads still retire on tokio's idle timeout.
pub const MAX_BLOCKING_THREADS: usize = openhuman_core::core::runtime::MAX_BLOCKING_THREADS;

/// Build a multi-thread runtime tuned for hosting agent turns.
///
/// Callers use this instead of `#[tokio::main]`, which offers no way to set the
/// worker stack size.
pub fn build_runtime() -> std::io::Result<tokio::runtime::Runtime> {
    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .thread_stack_size(WORKER_STACK_BYTES)
        .max_blocking_threads(MAX_BLOCKING_THREADS)
        .build()
}

#[cfg(test)]
mod tests {
    use super::*;

    // Both of these are facts about constants, so they are checked at compile
    // time rather than in a `#[test]`: a runtime assertion over a constant is
    // one clippy rejects, and a build failure beats a test failure for a value
    // that can only ever be wrong at authoring time.

    /// The whole point of this module. 2 MiB is tokio's default; anything near
    /// it means the tuning was lost and nested delegation will overflow.
    const _: () = assert!(
        WORKER_STACK_BYTES >= 16 * 1024 * 1024,
        "worker stack is too small to host a nested agent turn"
    );

    /// Bounded, but still above zero — an unbounded pool is what the tuning
    /// exists to prevent.
    const _: () = assert!(MAX_BLOCKING_THREADS > 0 && MAX_BLOCKING_THREADS < 512);

    #[test]
    fn build_runtime_produces_a_usable_runtime() {
        let rt = build_runtime().expect("runtime builds");
        assert_eq!(rt.block_on(async { 1 + 1 }), 2);
    }
}
