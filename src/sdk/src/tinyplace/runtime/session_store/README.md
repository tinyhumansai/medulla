# Session Store

Filesystem-backed `SessionStore` persistence for Signal ratchet/pre-key state, laid out to interoperate with the TS SDK's `FileSessionStore`.

## Contents

- [`mod.rs`](./mod.rs) — Filesystem-backed `SessionStore` persistence for Signal ratchet/pre-key state, laid out to interoperate with the TS SDK's `FileSessionStore`.
- [`types.rs`](./types.rs) — Data types for the `session_store` module.

## Maintenance

Keep this index synchronized when responsibilities move. Put shared data structures in `types.rs`, focused unit tests in `tests.rs` or a sibling `_tests.rs`, and preserve the module-level Rust documentation as the API source of truth.
