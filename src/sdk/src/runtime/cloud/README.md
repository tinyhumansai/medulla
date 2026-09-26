# Cloud

`Runtime` backed by the Medulla cloud backend, driving the orchestration API directly through `MedullaClient`. There is no embedded core in this path.

## Contents

- [`connect/`](./connect/) — Building the backend client this runtime drives, and deciding whether a host can actually use it (run, sign in, or stop).
- [`cell.rs`](./cell.rs) — The folded snapshot plus its change-notification channel.
- [`feedback.rs`](./feedback.rs) — The feedback board half of `CloudRuntime`.
- [`fold.rs`](./fold.rs) — Pure translation from backend wire types into the render snapshot.
- [`mod.rs`](./mod.rs) — `Runtime` backed by the Medulla cloud backend.
- [`tests.rs`](./tests.rs) — Unit tests for the cloud runtime.
- [`worker_ops.rs`](./worker_ops.rs) — Adapting hub workers and worker mutations to the runtime surface.

## Maintenance

Keep this index synchronized when responsibilities move. Put shared data structures in `types.rs`, focused unit tests in `tests.rs` or a sibling `_tests.rs`, and preserve the module-level Rust documentation as the API source of truth.
