//! Tokio runtime tuning for any process that may host an agent turn.
//!
//! These are process-level knobs, not SDK behaviour, but they live here rather
//! than in the app crate because every process that can host a turn needs them
//! and the SDK is what they all share.
//!
//! # These used to be re-exports
//!
//! They came from the embedded OpenHuman core, which is gone. Restating them is
//! the one honest option left: tinyagents publishes no equivalent, and a host
//! that silently took tokio's defaults would hit the failure below in front of
//! a user rather than in CI. The numbers are this crate's own now — change them
//! here, and say why.

/// Worker-thread stack size for a runtime that may host an agent turn.
///
/// tokio defaults worker threads to 2 MiB, which is not enough: an agent loop's
/// async chain is deep on its own, and a turn that delegates to a sub-agent
/// nests a second one inside the first — enough to overflow the default stack.
///
/// That failure mode is why this matters more than it looks: it reproduces only
/// under real nesting, so it surfaces in front of a user rather than in a unit
/// test, and it aborts the process rather than returning an error. 16 MiB is
/// the value the embedded core used and the same figure `cargo test` needs as
/// `RUST_MIN_STACK` for the suite's own 2 MiB test threads.
pub const WORKER_STACK_BYTES: usize = 16 * 1024 * 1024;

/// Upper bound on tokio's blocking-thread pool.
///
/// tokio defaults to 512. Blocking work in this process is bursty and bounded
/// (filesystem, sqlite, process spawn), so the default mostly buys idle
/// footprint. Threads still retire on tokio's idle timeout, so this is a
/// ceiling on a burst rather than a steady-state cost.
pub const MAX_BLOCKING_THREADS: usize = 64;

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
