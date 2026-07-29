# Registry

The session-binding registry: which harness session id a conversation is bound to, and the per-key serialization that keeps two turns from interleaving onto one transcript.

## Contents

- [`mod.rs`](./mod.rs) — The session-binding registry: which harness session id a conversation is bound to, and the per-key serialization that keeps two turns from interleaving onto one transcript.
- [`types.rs`](./types.rs) — Data types for the `registry` module.

## Maintenance

Keep this index synchronized when responsibilities move. Put shared data structures in `types.rs`, focused unit tests in `tests.rs` or a sibling `_tests.rs`, and preserve the module-level Rust documentation as the API source of truth.
