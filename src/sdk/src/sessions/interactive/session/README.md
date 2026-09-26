# Session

The live interactive harness process: spawn once, submit turns over stdin, read semantic events off stdout, interrupt a turn without killing the session, and close.

## Contents

- [`mod.rs`](./mod.rs) — The live interactive harness process: spawn once, submit turns over stdin, read semantic events off stdout, interrupt a turn without killing the session, and close.
- [`types.rs`](./types.rs) — Data types for the `session` module.

## Maintenance

Keep this index synchronized when responsibilities move. Put shared data structures in `types.rs`, focused unit tests in `tests.rs` or a sibling `_tests.rs`, and preserve the module-level Rust documentation as the API source of truth.
