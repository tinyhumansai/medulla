# System Info

Cheap, local worker capacity discovery.

## Contents

- [`mod.rs`](./mod.rs) — Cheap, local worker capacity discovery.
- [`tests.rs`](./tests.rs) — Deterministic parser and capture tests for worker system information.
- [`types.rs`](./types.rs) — Data types reported by a worker's lightweight system-information probe.

## Maintenance

Keep this index synchronized when responsibilities move. Put shared data structures in `types.rs`, focused unit tests in `tests.rs` or a sibling `_tests.rs`, and preserve the module-level Rust documentation as the API source of truth.
