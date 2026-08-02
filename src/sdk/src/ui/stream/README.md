# Stream

Pure derivations over a `RuntimeSnapshot`'s event and thread streams.

## Contents

- [`mod.rs`](./mod.rs) — Pure derivations over a `RuntimeSnapshot`'s event and thread streams.
- [`tests.rs`](./tests.rs) — Unit tests for the stream derivations.
- [`types.rs`](./types.rs) — Plain data types produced by the stream derivations in `super` — token-usage totals folded from the event log.

## Maintenance

Keep this index synchronized when responsibilities move. Put shared data structures in `types.rs`, focused unit tests in `tests.rs` or a sibling `_tests.rs`, and preserve the module-level Rust documentation as the API source of truth.
