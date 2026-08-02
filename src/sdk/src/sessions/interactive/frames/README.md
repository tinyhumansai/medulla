# Frames

The pure fold from `claude --output-format stream-json` frames to semantic `StreamEvent`s, and the stdin encoders for a turn and an interrupt.

## Contents

- [`mod.rs`](./mod.rs) — The pure fold from `claude --output-format stream-json` frames to semantic `StreamEvent`s, and the stdin encoders for a turn and an interrupt.
- [`types.rs`](./types.rs) — Data types for the `frames` module.

## Maintenance

Keep this index synchronized when responsibilities move. Put shared data structures in `types.rs`, focused unit tests in `tests.rs` or a sibling `_tests.rs`, and preserve the module-level Rust documentation as the API source of truth.
