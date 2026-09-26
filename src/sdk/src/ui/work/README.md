# Work

Rendering a `WorkSnapshot` as display rows: the goal, the todo list, the sub-agents, the files touched, and how the run ended.

## Contents

- [`mod.rs`](./mod.rs) — Rendering a `WorkSnapshot` as display rows: the goal, the todo list, the sub-agents, the files touched, and how the run ended.
- [`summary.rs`](./summary.rs) — The one-line forms of a work snapshot: the chip a list row can carry and the headline a pane header can show.
- [`tests.rs`](./tests.rs) — Unit tests for the work panel's rendering and its one-line summaries.

## Maintenance

Keep this index synchronized when responsibilities move. Put shared data structures in `types.rs`, focused unit tests in `tests.rs` or a sibling `_tests.rs`, and preserve the module-level Rust documentation as the API source of truth.
