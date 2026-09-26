# Chat Store

On-disk chat persistence for the Chat tab's thread trees.

## Contents

- [`mod.rs`](./mod.rs) — On-disk chat persistence for the Chat tab's thread trees.
- [`tests.rs`](./tests.rs) — Tests for the chat store module.
- [`types.rs`](./types.rs) — Data types for the `chat_store` module.

## Maintenance

Keep this index synchronized when responsibilities move. Put shared data structures in `types.rs`, focused unit tests in `tests.rs` or a sibling `_tests.rs`, and preserve the module-level Rust documentation as the API source of truth.
