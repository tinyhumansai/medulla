# Run

Running a workflow, and resuming one that paused.

## Contents

- [`mod.rs`](./mod.rs) — Running a workflow, and resuming one that paused.
- [`registry.rs`](./registry.rs) — The in-flight run registry.
- [`tests.rs`](./tests.rs) — Tests for running, pausing, resuming, and cancelling a workflow.

## Maintenance

Keep this index synchronized when responsibilities move. Put shared data structures in `types.rs`, focused unit tests in `tests.rs` or a sibling `_tests.rs`, and preserve the module-level Rust documentation as the API source of truth.
