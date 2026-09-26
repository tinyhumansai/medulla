# Headless

A non-interactive driver over the `Runtime` trait for scripting and end-to-end automation (a docker container, a CI probe): attach a runtime, submit exactly one instruction, stream the folded events to a writer as JSON lines, and return once the cycle result lands.

## Contents

- [`mod.rs`](./mod.rs) — A non-interactive driver over the `Runtime` trait for scripting and end-to-end automation (a docker container, a CI probe): attach a runtime, submit exactly one instruction, stream the folded events to a writer as JSON lines, and return once the cycle result lands.
- [`types.rs`](./types.rs) — Data types for the `headless` module.

## Maintenance

Keep this index synchronized when responsibilities move. Put shared data structures in `types.rs`, focused unit tests in `tests.rs` or a sibling `_tests.rs`, and preserve the module-level Rust documentation as the API source of truth.
