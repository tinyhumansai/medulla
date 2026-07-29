# Fleet

Pure view-model for the Fleet view: turn the declared capacity (`Host → Harness → Workspace → Agent`) and the agent-template catalog into a flattened row model plus pre-wrapped detail lines.

## Contents

- [`detail.rs`](./detail.rs) — Render the selected fleet node into pre-wrapped, styled detail rows.
- [`fmt.rs`](./fmt.rs) — Small formatters shared by the fleet rows and the detail pane: byte and token magnitudes, budget windows, and availability wording.
- [`mod.rs`](./mod.rs) — Pure view-model for the Fleet view: turn the declared capacity (`Host → Harness → Workspace → Agent`) and the agent-template catalog into a flattened row model plus pre-wrapped detail lines.
- [`registry.rs`](./registry.rs) — Project the local peer registry onto the containment chain.
- [`rows.rs`](./rows.rs) — Flatten the declared capacity and the roster into the Fleet list's rows.
- [`tests.rs`](./tests.rs) — Unit tests for the fleet view-model: the tree walk's shape and ordering, the dangling-parent and unplaced-agent fallbacks, budget selection, and the detail pane's placement resolution.
- [`types.rs`](./types.rs) — Data model for the Fleet view: the node kind, the flattened tree row, and their trivial classification impls. All behaviour lives in the sibling logic modules.

## Maintenance

Keep this index synchronized when responsibilities move. Put shared data structures in `types.rs`, focused unit tests in `tests.rs` or a sibling `_tests.rs`, and preserve the module-level Rust documentation as the API source of truth.
