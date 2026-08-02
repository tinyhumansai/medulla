# Clipboard

Clipboard writers: try a platform binary (pbcopy / clip / wl-copy / xclip / xsel) then fall back to OSC 52 (hand the text to the terminal). OSC 52 is the only mechanism that survives SSH, so it backstops rather than replaces the spawn path.

## Contents

- [`mod.rs`](./mod.rs) — Clipboard writers: try a platform binary (pbcopy / clip / wl-copy / xclip / xsel) then fall back to OSC 52 (hand the text to the terminal). OSC 52 is the only mechanism that survives SSH, so it backstops rather than replaces the spawn path.
- [`tests.rs`](./tests.rs) — Tests for the clipboard module.
- [`types.rs`](./types.rs) — Data types for the `clipboard` module.

## Maintenance

Keep this index synchronized when responsibilities move. Put shared data structures in `types.rs`, focused unit tests in `tests.rs` or a sibling `_tests.rs`, and preserve the module-level Rust documentation as the API source of truth.
