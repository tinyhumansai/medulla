# Service

The relay side of contact management: poll incoming requests into a `ContactBook`, apply the admission policy, and perform operator decisions.

## Contents

- [`mod.rs`](./mod.rs) — The relay side of contact management: poll incoming requests into a `ContactBook`, apply the admission policy, and perform operator decisions.
- [`types.rs`](./types.rs) — Data types for the `service` module.

## Maintenance

Keep this index synchronized when responsibilities move. Put shared data structures in `types.rs`, focused unit tests in `tests.rs` or a sibling `_tests.rs`, and preserve the module-level Rust documentation as the API source of truth.
