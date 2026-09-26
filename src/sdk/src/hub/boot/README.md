# Boot

Hub bootstrap: construct the remote tiny.place bridge + sender-runner, connect the Socket.IO harness client, and expose a live `HubHandle`.

## Contents

- [`mod.rs`](./mod.rs) — Hub bootstrap: construct the remote tiny.place bridge + sender-runner, connect the Socket.IO harness client, and expose a live `HubHandle`.
- [`types.rs`](./types.rs) — Data types for the `boot` module.

## Maintenance

Keep this index synchronized when responsibilities move. Put shared data structures in `types.rs`, focused unit tests in `tests.rs` or a sibling `_tests.rs`, and preserve the module-level Rust documentation as the API source of truth.
