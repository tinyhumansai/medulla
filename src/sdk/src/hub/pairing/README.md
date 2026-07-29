# Pairing

Inbound pairing for the hub's own tiny.place identity.

## Contents

- [`mod.rs`](./mod.rs) — Inbound pairing for the hub's own tiny.place identity.
- [`tests.rs`](./tests.rs) — Inbound pairing: what the hub identity does with a contact request it never asked for.
- [`types.rs`](./types.rs) — Data types for the `pairing` module.

## Maintenance

Keep this index synchronized when responsibilities move. Put shared data structures in `types.rs`, focused unit tests in `tests.rs` or a sibling `_tests.rs`, and preserve the module-level Rust documentation as the API source of truth.
