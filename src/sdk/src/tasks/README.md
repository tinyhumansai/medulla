# Tasks

Durable local tasks and provider configuration.

## Contents

- [`github/`](./github/) — GitHub task source adapter using the public REST API.
- [`mod.rs`](./mod.rs) — Durable local tasks and provider configuration.
- [`tests.rs`](./tests.rs) — Focused tests for local task persistence, synchronization, and recurrence.
- [`types.rs`](./types.rs) — Data types for the `tasks` module.

## Maintenance

Keep this index synchronized when responsibilities move. Put shared data structures in `types.rs`, focused unit tests in `tests.rs` or a sibling `_tests.rs`, and preserve the module-level Rust documentation as the API source of truth.
