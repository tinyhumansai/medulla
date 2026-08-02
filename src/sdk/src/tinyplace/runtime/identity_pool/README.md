# Identity Pool

Collision-free identity acquisition for the daemon.

## Contents

- [`mod.rs`](./mod.rs) — Collision-free identity acquisition for the daemon.
- [`tests.rs`](./tests.rs) — Offline, deterministic coverage for identity-slot acquisition.
- [`types.rs`](./types.rs) — Data model for identity-slot acquisition: the lock guard that makes a slot exclusive, and the bundle handed back to the daemon once one is held.

## Maintenance

Keep this index synchronized when responsibilities move. Put shared data structures in `types.rs`, focused unit tests in `tests.rs` or a sibling `_tests.rs`, and preserve the module-level Rust documentation as the API source of truth.
