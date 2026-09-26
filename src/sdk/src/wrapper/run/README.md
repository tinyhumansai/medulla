# Run

Process orchestration for a wrapped session: the `run_wrapper` entry point, the `run_wrapper_with` core loop that drives the child CLI and the tiny.place `Bridge`, and the exit-code / signal plumbing around it.

## Contents

- [`child/`](./child/) — Spawning the harness child, and the uniform handle the run loop drives it through.
- [`tests/`](./tests/) — Unit tests for child spawning: which stdio strategy is chosen, and how the PTY handles are wired through to the run loop.
- [`mod.rs`](./mod.rs) — Process orchestration for a wrapped session: the `run_wrapper` entry point, the `run_wrapper_with` core loop that drives the child CLI and the tiny.place `Bridge`, and the exit-code / signal plumbing around it.

## Maintenance

Keep this index synchronized when responsibilities move. Put shared data structures in `types.rs`, focused unit tests in `tests.rs` or a sibling `_tests.rs`, and preserve the module-level Rust documentation as the API source of truth.
