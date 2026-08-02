# Harness Contract

Public agent-harness wire-contract types.

## Contents

- [`mod.rs`](./mod.rs) — Public agent-harness wire-contract types.
- [`tests.rs`](./tests.rs) — Round-trip serde tests against hand-written public-contract JSON literals. Each test asserts the exact camelCase / lowercase field and tag names, so a wire-incompatible rename fails here.
- [`types.rs`](./types.rs) — Serde data types for the public agent-harness wire shapes. Field renames pin every struct to the JSON contract (`rename_all = "camelCase"`); enum renames pin the lowercase status/state strings used on the wire. Only shapes and their trivial impls live here — the reserved tool-name vocabulary and re-exports live in the parent module.

## Maintenance

Keep this index synchronized when responsibilities move. Put shared data structures in `types.rs`, focused unit tests in `tests.rs` or a sibling `_tests.rs`, and preserve the module-level Rust documentation as the API source of truth.
