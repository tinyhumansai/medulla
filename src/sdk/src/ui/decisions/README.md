# Decisions

Prepared operator decisions derived from harness escalations and pending worker questions. The fold is UI-agnostic so terminal and future hosts share stable ids, ordering, deduplication, and answer routing.

## Contents

- [`fold.rs`](./fold.rs) — Deterministic folding of current harness/lane state into prepared decisions.
- [`mod.rs`](./mod.rs) — Prepared operator decisions derived from harness escalations and pending worker questions. The fold is UI-agnostic so terminal and future hosts share stable ids, ordering, deduplication, and answer routing.
- [`tests.rs`](./tests.rs) — Decision-fold tests for ordering, dedupe, and answered-item removal.
- [`types.rs`](./types.rs) — Data shapes for the prepared-decision queue.

## Maintenance

Keep this index synchronized when responsibilities move. Put shared data structures in `types.rs`, focused unit tests in `tests.rs` or a sibling `_tests.rs`, and preserve the module-level Rust documentation as the API source of truth.
