# Harness Work

What a coding-agent harness is *working on*, in one vocabulary.

## Contents

- [`fold.rs`](./fold.rs) — The fold: turn a session's semantic-event stream into a live `WorkSnapshot`.
- [`mod.rs`](./mod.rs) — What a coding-agent harness is *working on*, in one vocabulary.
- [`tests.rs`](./tests.rs) — Unit tests for the work fold and its data model.
- [`types.rs`](./types.rs) — The provider-neutral work model: what every coding-agent harness shows on its own screen, reduced to one vocabulary.

## Maintenance

Keep this index synchronized when responsibilities move. Put shared data structures in `types.rs`, focused unit tests in `tests.rs` or a sibling `_tests.rs`, and preserve the module-level Rust documentation as the API source of truth.
