# Openhuman

`Runtime` backed by the embedded OpenHuman core.

## Contents

- [`cell.rs`](./cell.rs) — The folded snapshot plus its change-notification channel.
- [`fold.rs`](./fold.rs) — Pure translation from core wire types into the render snapshot.
- [`mod.rs`](./mod.rs) — `Runtime` backed by the embedded OpenHuman core.
- [`tests.rs`](./tests.rs) — Unit tests for the embedded-core runtime.
- [`worker_ops.rs`](./worker_ops.rs) — Adapting hub workers and worker mutations to the embedded runtime surface.

## Maintenance

Keep this index synchronized when responsibilities move. Put shared data structures in `types.rs`, focused unit tests in `tests.rs` or a sibling `_tests.rs`, and preserve the module-level Rust documentation as the API source of truth.
