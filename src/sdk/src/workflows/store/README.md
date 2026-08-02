# Store

Where workflows and their run records live.

## Contents

- [`file.rs`](./file.rs) — JSON workflow documents under `.medulla/workflows`, one graph per file.
- [`mod.rs`](./mod.rs) — Where workflows and their run records live.
- [`tests.rs`](./tests.rs) — Unit tests for workflow directory layering, document parsing, and the file-backed store's read/write/delete and run-history behaviour.

## Maintenance

Keep this index synchronized when responsibilities move. Put shared data structures in `types.rs`, focused unit tests in `tests.rs` or a sibling `_tests.rs`, and preserve the module-level Rust documentation as the API source of truth.
