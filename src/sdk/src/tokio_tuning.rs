//! Tokio sizing shared with the embed runtime that executes native turns.

pub use openhuman_embed::process::{
    AGENT_WORKER_STACK_BYTES as WORKER_STACK_BYTES, MAX_BLOCKING_THREADS,
};

/// Build a runtime sized for embedded agent turns and nested delegation.
pub fn build_runtime() -> std::io::Result<tokio::runtime::Runtime> {
    openhuman_embed::process::tokio_runtime_builder().build()
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
