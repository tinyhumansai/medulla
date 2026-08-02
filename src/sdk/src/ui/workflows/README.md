# Workflows

The UI-facing view of installed workflows: their listings, their graphs, and their runs.

## Contents

- [`copilot_tests.rs`](./copilot_tests.rs) — Tests for the copilot transcript model.
- [`copilot.rs`](./copilot.rs) — The transcript model for the workflow copilot.
- [`graph_tests.rs`](./graph_tests.rs) — Tests for the terminal graph layout and its cursor.
- [`graph.rs`](./graph.rs) — Laying a workflow graph out for a terminal, and moving a cursor through it.
- [`inspect_tests.rs`](./inspect_tests.rs) — Tests for the node inspector and the run overlay.
- [`inspect.rs`](./inspect.rs) — What a selected node says about itself, and how a run marks up the graph.
- [`mod.rs`](./mod.rs) — The UI-facing view of installed workflows: their listings, their graphs, and their runs.
- [`progress_tests.rs`](./progress_tests.rs) — Tests for progress-frame classification.
- [`progress.rs`](./progress.rs) — Reading a harness progress frame as the kind of transcript line it is.
- [`rows_tests.rs`](./rows_tests.rs) — Tests for the workflow row builders.
- [`rows.rs`](./rows.rs) — Listing rows for installed workflows and their runs.

## Maintenance

Keep this index synchronized when responsibilities move. Put shared data structures in `types.rs`, focused unit tests in `tests.rs` or a sibling `_tests.rs`, and preserve the module-level Rust documentation as the API source of truth.
