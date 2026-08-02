# Listener

Envelopes delivered over the relay's push channel, instead of fetched.

## Contents

- [`mod.rs`](./mod.rs) — Envelopes delivered over the relay's push channel, instead of fetched.
- [`tests.rs`](./tests.rs) — Unit tests for the push inbox: what the socket's frames decode to, what the delivery queue guarantees, and the deduplication that makes redelivery safe.

## Maintenance

Keep this index synchronized when responsibilities move. Put shared data structures in `types.rs`, focused unit tests in `tests.rs` or a sibling `_tests.rs`, and preserve the module-level Rust documentation as the API source of truth.
