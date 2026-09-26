# Command

Slash-command parsing, the command catalog, and the `/copy` transcript helper.

## Contents

- [`catalog.rs`](./catalog.rs) — The slash-command catalog: every command, its aliases, its argument hint, and the one line that describes it.
- [`mod.rs`](./mod.rs) — Slash-command parsing, the command catalog, and the `/copy` transcript helper.
- [`tests.rs`](./tests.rs) — Unit tests for slash-command parsing and the `/copy` helper.
- [`types.rs`](./types.rs) — The parsed slash-command vocabulary and the clipboard scope it can carry.

## Maintenance

Keep this index synchronized when responsibilities move. Put shared data structures in `types.rs`, focused unit tests in `tests.rs` or a sibling `_tests.rs`, and preserve the module-level Rust documentation as the API source of truth.
