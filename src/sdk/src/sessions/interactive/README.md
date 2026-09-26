# Interactive

The interactive transport: one long-lived harness process fed newline- delimited JSON turns over stdin.

## Contents

- [`frames/`](./frames/) — The pure fold from `claude --output-format stream-json` frames to semantic `StreamEvent`s, and the stdin encoders for a turn and an interrupt.
- [`session/`](./session/) — The live interactive harness process: spawn once, submit turns over stdin, read semantic events off stdout, interrupt a turn without killing the session, and close.
- [`mod.rs`](./mod.rs) — The interactive transport: one long-lived harness process fed newline- delimited JSON turns over stdin.
- [`tests.rs`](./tests.rs) — Tests for the interactive transport: the pure frame fold and stdin encoders, plus the live process driven by a fake `/bin/sh` harness (spawn, turn loop, interrupt, and teardown) so the plumbing is pinned without a real `claude`.

## Maintenance

Keep this index synchronized when responsibilities move. Put shared data structures in `types.rs`, focused unit tests in `tests.rs` or a sibling `_tests.rs`, and preserve the module-level Rust documentation as the API source of truth.
