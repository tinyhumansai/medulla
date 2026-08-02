# Roster

The hub's worker-roster data: the shared roster type, the `AgentDescriptor` payload the hub advertises, and the address resolution the socket layer uses to target a task. Pure and offline-testable; the live control handle that mutates the roster over the Socket.IO uplink lives in `handle`.

## Contents

- [`mod.rs`](./mod.rs) — The hub's worker-roster data: the shared roster type, the `AgentDescriptor` payload the hub advertises, and the address resolution the socket layer uses to target a task. Pure and offline-testable; the live control handle that mutates the roster over the Socket.IO uplink lives in `handle`.
- [`types.rs`](./types.rs) — Data types for the `roster` module.

## Maintenance

Keep this index synchronized when responsibilities move. Put shared data structures in `types.rs`, focused unit tests in `tests.rs` or a sibling `_tests.rs`, and preserve the module-level Rust documentation as the API source of truth.
