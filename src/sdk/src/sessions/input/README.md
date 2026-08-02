# Input

**The driver seam.** Sessions are driven from exactly two sources — a `medulla-tinyplace/1` task frame, or a `tinyplace.harness.session.v*` envelope — and this module is the one place that knows the difference.

## Contents

- [`mod.rs`](./mod.rs) — **The driver seam.** Sessions are driven from exactly two sources — a `medulla-tinyplace/1` task frame, or a `tinyplace.harness.session.v*` envelope — and this module is the one place that knows the difference.
- [`types.rs`](./types.rs) — Data types for the `input` module.

## Maintenance

Keep this index synchronized when responsibilities move. Put shared data structures in `types.rs`, focused unit tests in `tests.rs` or a sibling `_tests.rs`, and preserve the module-level Rust documentation as the API source of truth.
