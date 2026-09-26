# Tests

Unit tests for the Agents view-model, split by responsibility: `fold` covers the event fold and Agents-list row model; `render` covers status/role classification, key parsing, and transcript rendering; `roster` covers the worker-registry merge that feeds the fold.

## Contents

- [`activity.rs`](./activity.rs) — Unit tests for folding locally-observed worker activity into lanes.
- [`fold.rs`](./fold.rs) — Tests for the event fold and the Agents-list row model.
- [`mod.rs`](./mod.rs) — Unit tests for the Agents view-model, split by responsibility: `fold` covers the event fold and Agents-list row model; `render` covers status/role classification, key parsing, and transcript rendering; `roster` covers the worker-registry merge that feeds the fold.
- [`render.rs`](./render.rs) — Tests for status/role classification, key parsing, task ordering, and transcript rendering (plus the shared formatting helpers).
- [`roster.rs`](./roster.rs) — Unit tests for the worker-registry → roster merge.

## Maintenance

Keep this index synchronized when responsibilities move. Put shared data structures in `types.rs`, focused unit tests in `tests.rs` or a sibling `_tests.rs`, and preserve the module-level Rust documentation as the API source of truth.
