# Event Log

Shared bounded event storage for runtime conversation threads.

## Contents

- [`mod.rs`](./mod.rs) — Shared bounded event storage for runtime conversation threads.
- [`tests.rs`](./tests.rs) — Tests for the shared runtime event-log retention and projection policy.
- [`types.rs`](./types.rs) — The bounded per-thread event-log type and its retention policy.

## Maintenance

Keep this index synchronized when responsibilities move. Put shared data structures in `types.rs`, focused unit tests in `tests.rs` or a sibling `_tests.rs`, and preserve the module-level Rust documentation as the API source of truth.
