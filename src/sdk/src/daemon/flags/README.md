# Flags

Command-line flag parsing for `medulla daemon`: the permissive `Flags` tokenizer (values, repeatable comma-lists, and boolean switches) and `parse_provider`, the wire-name → `HarnessProvider` mapper. Consumed by `super::entry` to build the daemon configuration.

## Contents

- [`mod.rs`](./mod.rs) — Command-line flag parsing for `medulla daemon`: the permissive `Flags` tokenizer (values, repeatable comma-lists, and boolean switches) and `parse_provider`, the wire-name → `HarnessProvider` mapper. Consumed by `super::entry` to build the daemon configuration.
- [`types.rs`](./types.rs) — Data types for the `flags` module.

## Maintenance

Keep this index synchronized when responsibilities move. Put shared data structures in `types.rs`, focused unit tests in `tests.rs` or a sibling `_tests.rs`, and preserve the module-level Rust documentation as the API source of truth.
