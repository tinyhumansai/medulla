# Book

`ContactBook` — the pending-request queue the operator works through, and the policy that decides which requests never reach them.

## Contents

- [`mod.rs`](./mod.rs) — `ContactBook` — the pending-request queue the operator works through, and the policy that decides which requests never reach them.
- [`types.rs`](./types.rs) — Data types for the `book` module.

## Maintenance

Keep this index synchronized when responsibilities move. Put shared data structures in `types.rs`, focused unit tests in `tests.rs` or a sibling `_tests.rs`, and preserve the module-level Rust documentation as the API source of truth.
