# Copilot

The workflow copilot: a harness turn scoped to one graph.

## Contents

- [`diff_tests.rs`](./diff_tests.rs) — Tests for the before/after graph difference.
- [`diff.rs`](./diff.rs) — What changed between two versions of a graph.
- [`mod.rs`](./mod.rs) — The workflow copilot: a harness turn scoped to one graph.
- [`prompt_tests.rs`](./prompt_tests.rs) — Tests for the copilot prompt.
- [`prompt.rs`](./prompt.rs) — The instruction the copilot harness is given.
- [`tests.rs`](./tests.rs) — Tests for one copilot turn.

## Maintenance

Keep this index synchronized when responsibilities move. Put shared data structures in `types.rs`, focused unit tests in `tests.rs` or a sibling `_tests.rs`, and preserve the module-level Rust documentation as the API source of truth.
