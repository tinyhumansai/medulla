# Budget

Best-effort, fail-open budget and readiness probe for the installed harnesses.

## Contents

- [`mod.rs`](./mod.rs) — Best-effort, fail-open budget and readiness probe for the installed harnesses.
- [`tests.rs`](./tests.rs) — Tests for the fail-open budget/readiness probe core and its seam driver.
- [`types.rs`](./types.rs) — Data types for the budget/readiness probe: the operator-declared numbers, the pure core's per-provider input, and the injectable environment seams. Behaviour-heavy `impl`s (seam wiring, evaluation) live beside the logic in `super`.

## Maintenance

Keep this index synchronized when responsibilities move. Put shared data structures in `types.rs`, focused unit tests in `tests.rs` or a sibling `_tests.rs`, and preserve the module-level Rust documentation as the API source of truth.
