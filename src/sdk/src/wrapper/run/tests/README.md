# Tests

Unit tests for child spawning: which stdio strategy is chosen, and how the PTY handles are wired through to the run loop.

## Contents

- [`mod.rs`](./mod.rs) — Unit tests for child spawning: which stdio strategy is chosen, and how the PTY handles are wired through to the run loop.
- [`types.rs`](./types.rs) — Test-only data types for wrapper child spawning.

## Maintenance

Keep this index synchronized when responsibilities move. Put shared data structures in `types.rs`, focused unit tests in `tests.rs` or a sibling `_tests.rs`, and preserve the module-level Rust documentation as the API source of truth.
