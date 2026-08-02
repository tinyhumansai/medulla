# Flow Engine

The adapter seam between Medulla and the `tinyflows` workflow engine.

## Contents

- [`caps/`](./caps/) — Assembling the capability bundle the engine runs against.
- [`execute.rs`](./execute.rs) — Driving the engine: compile, run, resume, simulate.
- [`harness_choice_tests.rs`](./harness_choice_tests.rs) — Unit tests for harness and model selection.
- [`harness_choice.rs`](./harness_choice.rs) — Which harness and model an `agent` node runs on.
- [`harness_selection_tests.rs`](./harness_selection_tests.rs) — Tests for which harness and model an `agent` node's dispatch carries.
- [`mod.rs`](./mod.rs) — The adapter seam between Medulla and the `tinyflows` workflow engine.
- [`observability_tests.rs`](./observability_tests.rs) — Tests for the run observer.
- [`observability.rs`](./observability.rs) — Turning engine run callbacks into Medulla's own work events.
- [`settings_tests.rs`](./settings_tests.rs) — Tests for capability settings and the config that produces them.
- [`settings.rs`](./settings.rs) — What the capability adapters are allowed to do, and where they keep things.
- [`tests.rs`](./tests.rs) — Tests for the capability seam.

## Maintenance

Keep this index synchronized when responsibilities move. Put shared data structures in `types.rs`, focused unit tests in `tests.rs` or a sibling `_tests.rs`, and preserve the module-level Rust documentation as the API source of truth.
