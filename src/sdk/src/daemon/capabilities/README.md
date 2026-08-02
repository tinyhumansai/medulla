# Capabilities

Capability discovery for supported local harnesses.

## Contents

- [`budget/`](./budget/) — Best-effort, fail-open budget and readiness probe for the installed harnesses.
- [`mod.rs`](./mod.rs) — Capability discovery for supported local harnesses.
- [`tests.rs`](./tests.rs) — Tests for the capabilities module.
- [`types.rs`](./types.rs) — Data types for the `capabilities` module.

## Maintenance

Keep this index synchronized when responsibilities move. Put shared data structures in `types.rs`, focused unit tests in `tests.rs` or a sibling `_tests.rs`, and preserve the module-level Rust documentation as the API source of truth.
