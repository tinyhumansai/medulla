# Manager Tests

Manager tests: the session lifecycle, the bounded/unbound turn split, and the transcript the Sessions tab renders.

## Contents

- [`lifecycle.rs`](./lifecycle.rs) — Session lifecycle: open, reopen, attachability, reset, close, forget, observe.
- [`mod.rs`](./mod.rs) — Manager tests: the session lifecycle, the bounded/unbound turn split, and the transcript the Sessions tab renders.
- [`turns.rs`](./turns.rs) — Turn execution: the bounded/unbound split, capture-and-resume, and failure survival.

## Maintenance

Keep this index synchronized when responsibilities move. Put shared data structures in `types.rs`, focused unit tests in `tests.rs` or a sibling `_tests.rs`, and preserve the module-level Rust documentation as the API source of truth.
