# Clipboard

Clipboard writers: the surrounding tmux's paste buffer, a platform binary (pbcopy / clip / wl-copy / xclip / xsel), and OSC 52 (hand the text to the terminal). OSC 52 is the only mechanism that survives SSH, so it backstops rather than replaces the spawn path — and `tmux.rs` is what keeps it working when the SSH session lands in a multiplexer instead of a terminal.

## Contents

- [`mod.rs`](./mod.rs) — The writers themselves and the two copy paths: local-first (`copy_to_clipboard`) and terminal-first (`copy_for_operator`).
- [`tmux.rs`](./tmux.rs) — tmux awareness: DCS passthrough out of a nested tmux, the `load-buffer` hop, and reading a child's OSC 52 back out of its parameters.
- [`tests.rs`](./tests.rs) — Tests for the clipboard module.
- [`types.rs`](./types.rs) — Data types for the `clipboard` module.

## Maintenance

Keep this index synchronized when responsibilities move. Put shared data structures in `types.rs`, focused unit tests in `tests.rs` or a sibling `_tests.rs`, and preserve the module-level Rust documentation as the API source of truth.
